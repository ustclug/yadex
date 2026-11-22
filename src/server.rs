use std::{
    env::set_current_dir,
    fs, io,
    os::unix::fs::{MetadataExt, chroot},
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Router,
    extract::{Json, State},
    http::{HeaderValue, StatusCode, Uri, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use chrono::{TimeZone, Utc};
use futures_util::StreamExt as SExt;
use handlebars::{RenderError, handlebars_helper};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use tokio::{fs::DirEntry, net::TcpListener};
use tokio_stream::wrappers::ReadDirStream;
use tracing::error;

use crate::config::{ServiceConfig, TemplateConfig};

pub struct App {}

#[derive(Default)]
pub struct Template {
    registry: handlebars::Handlebars<'static>,
}

#[derive(Debug, Snafu)]
pub enum TemplateLoadError {
    #[snafu(display("failed to load {component} template from {path:?}: {source}"))]
    Io {
        path: PathBuf,
        source: std::io::Error,
        component: &'static str,
    },
    Register {
        component: &'static str,
        source: handlebars::TemplateError,
    },
}

// Note: This gives UTC time to frontend. It's expected that frontend shall convert to users' timezone, if necessary.
handlebars_helper!(from_mtimestamp_helper: |t: i64| {
    match chrono::DateTime::from_timestamp(t, 0) {
        Some(dt) => Utc
            .timestamp_opt(dt.timestamp(), 0)
            .single()
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "Invalid timestamp".to_string()),
        None => "Invalid timestamp".to_string(),
    }
});

handlebars_helper!(humanize_size_helper: |s: u64| {
    if s >= 1 << 30 {
        format!("{:.2} GiB", s as f64 / (1 << 30) as f64)
    } else if s >= 1 << 20 {
        format!("{:.2} MiB", s as f64 / (1 << 20) as f64)
    } else if s >= 1 << 10 {
        format!("{:.2} KiB", s as f64 / (1 << 10) as f64)
    } else {
        format!("{} B", s)
    }
});

impl Template {
    pub fn from_config(
        path_to_config: &Path,
        config: TemplateConfig,
    ) -> Result<Self, TemplateLoadError> {
        let mut registry = handlebars::Handlebars::new();
        let config_dir = path_to_config.parent().unwrap();
        let index_path = config_dir.join(config.index_file);
        let index = std::fs::read_to_string(&index_path).context(IoSnafu {
            component: "index",
            path: index_path,
        })?;
        registry
            .register_template_string("index", index)
            .context(RegisterSnafu { component: "index" })?;
        registry.register_helper("from_mtimestamp", Box::new(from_mtimestamp_helper));
        registry.register_helper("humanize_size", Box::new(humanize_size_helper));
        Ok(Self { registry })
    }

    pub fn render<T>(&self, name: &str, data: &T) -> Result<String, RenderError>
    where
        T: Serialize,
    {
        self.registry.render(name, data)
    }
}

impl App {
    pub async fn serve(
        config: ServiceConfig,
        listener: TcpListener,
        template: Template,
    ) -> Result<(), YadexError> {
        let confined_root = if config.security == crate::config::Security::Path {
            Some(Arc::new(
                crate::confined::Root::new(&config.root)
                    .whatever_context("failed to initialize openat2 confinement")?,
            ))
        } else {
            None
        };
        let root: &'static Path = Box::leak(Box::<Path>::from(config.root));
        if config.security == crate::config::Security::Chroot {
            chroot(root).whatever_context("failed to chroot")?;
            set_current_dir("/").whatever_context("failed to cd into new root")?;
        } else {
            set_current_dir(root).whatever_context("failed to cd into given path")?;
        }
        let mut router = Router::new();
        if config.template_index {
            router = router.fallback(get(directory_listing));
        }
        if config.json_api {
            router = router.route("/api/files", post(api_directory_listing));
        }
        let router = router.with_state(AppState {
            confined_root,
            limit: if config.limit == 0 {
                usize::MAX
            } else {
                config.limit as usize
            },
            template: Arc::new(template),
        });
        sd_notify::notify(true, &[sd_notify::NotifyState::Ready])
            .whatever_context("failed to do systemd notify")?;
        axum::serve(listener, router)
            .await
            .with_whatever_context(|_| "serve failed")
    }
}

#[derive(Clone)]
pub struct AppState {
    confined_root: Option<Arc<crate::confined::Root>>,
    limit: usize,
    template: Arc<Template>,
}

#[derive(Debug, Clone, Serialize)]
struct DirEntryInfo {
    name: String,
    is_dir: bool,
    size: u64,
    href: String,
    datetime: i64,
}

pub async fn direntry_info(val: Result<DirEntry, io::Error>) -> Option<(DirEntry, fs::Metadata)> {
    let val = val.ok()?;
    // We need to get the actual metadata (not symlink metadata) here
    let meta = tokio::fs::metadata(val.path()).await.ok()?;
    Some((val, meta))
}

#[derive(Debug, Clone, Serialize)]
struct IndexData<'a> {
    entry: &'a [DirEntryInfo],
    maybe_truncated: bool,
    cwd: &'a str,
}

/// Maps a (percent-decoded) request path to a path below `base`.
///
/// Returns `None` if the path contains a `..` segment or a NUL byte. A `..` segment
/// must never be silently dropped: a front-end web server (nginx) *resolves* it before
/// it matches locations and applies its access rules, so "ignoring" it here would make
/// yadex list a different directory than the one the front end has authorised
/// (`/private/../` is `/` for nginx, but would be `private/` for yadex).
fn to_relative(base: &Path, path: &str) -> Option<PathBuf> {
    if path.contains('\0') {
        return None;
    }
    let mut safe_path = PathBuf::from(base);
    let p = Path::new(path);

    for comp in p.components() {
        use std::path::Component;
        match comp {
            Component::RootDir => {}             // No absolute paths
            Component::ParentDir => return None, // No going up, and no guessing either
            Component::CurDir => {}              // Ignore redundant current dir
            Component::Prefix(_) => unreachable!("yadex does not support Windows"),
            Component::Normal(comp) => safe_path.push(comp),
        }
    }

    Some(safe_path)
}

/// The error returned for request paths that `to_relative` refuses.
fn invalid_path() -> YadexError {
    YadexError::NotFound {
        source: std::io::ErrorKind::InvalidInput.into(),
    }
}

fn path_to_href(path: &Path) -> String {
    let mut segments = Vec::new();
    for comp in path.components() {
        if comp == std::path::Component::CurDir {
            continue;
        }
        let seg = comp.as_os_str().to_string_lossy();
        let escaped = urlencoding::encode(&seg).to_string();
        segments.push(escaped);
    }
    format!("/{}", segments.join("/"))
}

/// The "add the trailing slash" redirect for `path` (as returned by `to_relative`).
///
/// The target is never built from the raw or the percent-decoded request path: it is
/// derived from the sanitised path and percent-encoded again by `path_to_href`. It
/// therefore always starts with exactly one `/` (never a scheme-relative `//host`,
/// `/\host` or `/<TAB>/host`), only consists of URL-safe ASCII (always a valid header
/// value, nothing is decoded twice by the next hop) and keeps the query string.
fn redirect_to_directory(path: &Path, query: Option<&str>) -> Response {
    let mut location = path_to_href(path);
    if !location.ends_with('/') {
        location.push('/');
    }
    if let Some(query) = query {
        location.push('?');
        location.push_str(query);
    }
    match HeaderValue::try_from(location) {
        Ok(location) => (
            StatusCode::PERMANENT_REDIRECT,
            [(header::LOCATION, location)],
        )
            .into_response(),
        // Cannot happen (see above) - but request data must never be able to cause a panic.
        Err(_) => invalid_path().into_response(),
    }
}

fn remove_first_component<P: AsRef<Path>>(path: P) -> PathBuf {
    let mut comps = path.as_ref().components();
    comps.next();
    let rest = comps.as_path();
    if rest.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        rest.to_path_buf()
    }
}

async fn get_entries(
    path: &Path,
    limit: usize,
    sort: bool,
    confined_root: Option<&Arc<crate::confined::Root>>,
) -> Result<Vec<DirEntryInfo>, YadexError> {
    let raw_entries = if let Some(root) = confined_root {
        let root = Arc::clone(root);
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || root.entries(&path, limit))
            .await
            .whatever_context("directory listing task failed")?
            .context(NotFoundSnafu)?
    } else {
        ReadDirStream::new(tokio::fs::read_dir(path).await.context(NotFoundSnafu)?)
            .take(limit)
            .filter_map(async |entry| {
                direntry_info(entry)
                    .await
                    .map(|(d, meta)| (d.file_name(), meta))
            })
            .collect::<Vec<_>>()
            .await
    };
    let mut entries = raw_entries
        .into_iter()
        .filter_map(|(name, meta)| {
            let displayed_name = name.to_string_lossy();
            if displayed_name.starts_with('.') {
                return None;
            }
            Some(DirEntryInfo {
                is_dir: meta.is_dir(),
                size: if meta.is_dir() { 0 } else { meta.size() },
                href: format!(
                    "{href}{slash}",
                    href = path_to_href(&path.join(&name)),
                    slash = if meta.is_dir() { "/" } else { "" }
                ),
                name: displayed_name.into_owned(),
                datetime: meta.mtime(),
            })
        })
        .collect::<Vec<_>>();
    if sort {
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });
    }
    Ok(entries)
}

#[derive(Deserialize)]
pub struct APIInput {
    path: String,
}

#[derive(Serialize)]
pub struct APIOutput {
    entries: Vec<DirEntryInfo>,
    maybe_truncated: bool,
}

#[axum::debug_handler]
pub async fn api_directory_listing(
    State(state): State<AppState>,
    Json(payload): Json<APIInput>,
) -> Result<Json<APIOutput>, YadexError> {
    let mut path = payload.path;
    if !path.ends_with('/') {
        path.push('/');
    }
    let path = to_relative(Path::new("."), &path).ok_or_else(invalid_path)?;
    let path = path.as_path();
    tracing::debug!("API listing directory: {:?}", path);

    let entries = get_entries(path, state.limit, false, state.confined_root.as_ref()).await?;
    let maybe_truncated = entries.len() == state.limit;
    let output = APIOutput {
        entries,
        maybe_truncated,
    };
    Ok(Json(output))
}

#[axum::debug_handler]
pub async fn directory_listing(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Response, YadexError> {
    let path = uri.path();

    // decode
    let path = urlencoding::decode(path)
        .map_err(|_| YadexError::NotFound {
            source: std::io::ErrorKind::NotADirectory.into(),
        })?
        .into_owned();

    let ends_with_slash = path.ends_with('/');
    let path = to_relative(Path::new("."), &path).ok_or_else(invalid_path)?;
    let path = path.as_path();

    if !ends_with_slash {
        return Ok(redirect_to_directory(path, uri.query()));
    }
    tracing::debug!("listing directory: {:?}", path);

    let entries = get_entries(path, state.limit, true, state.confined_root.as_ref()).await?;
    let html = state
        .template
        .render(
            "index",
            &IndexData {
                entry: &entries,
                maybe_truncated: entries.len() == state.limit,
                cwd: remove_first_component(path).display().to_string().as_str(),
            },
        )
        .context(RenderSnafu { template: "index" })?;
    Ok(Html(html).into_response())
}

#[derive(Debug, Snafu)]
pub enum YadexError {
    #[snafu(display("The resource you are requesting does not exist"))]
    NotFound { source: std::io::Error },
    #[snafu(whatever, display("{message}"))]
    Whatever {
        #[snafu(source(from(color_eyre::Report, Some)))]
        source: Option<color_eyre::Report>,
        message: String,
    },
    #[snafu(display("The template {template} failed to render"))]
    Render {
        source: RenderError,
        template: &'static str,
    },
}

/// Whether a failed directory lookup means "there is no such (listable) directory",
/// as opposed to an operational problem on the server side.
fn is_not_found(err: &io::Error) -> bool {
    use rustix::io::Errno;
    match err.kind() {
        io::ErrorKind::NotFound
        | io::ErrorKind::NotADirectory
        | io::ErrorKind::PermissionDenied
        | io::ErrorKind::InvalidInput => true,
        // A path that leaves the root (openat2 with RESOLVE_BENEATH), a symlink
        // loop, an over-long name.
        _ => [Errno::XDEV, Errno::LOOP, Errno::NAMETOOLONG]
            .iter()
            .any(|errno| err.raw_os_error() == Some(errno.raw_os_error())),
    }
}

impl IntoResponse for YadexError {
    fn into_response(self) -> Response {
        match &self {
            YadexError::NotFound { source } if is_not_found(source) => {
                (StatusCode::NOT_FOUND, "404 Not Found").into_response()
            }
            YadexError::NotFound { source } => {
                // EMFILE, ENFILE, ENOMEM, EIO, EAGAIN, ...: the directory may well exist.
                // Do not tell clients and caches that it does not.
                error!("failed to list directory: {source}");
                (StatusCode::SERVICE_UNAVAILABLE, "503 Service Unavailable").into_response()
            }
            YadexError::Whatever { source, message } => {
                error!("internal error: {message}, source: {source:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
            }
            YadexError::Render { source, .. } => {
                error!("internal error: {self}, source: {source:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_relative_refuses_dot_dot_instead_of_dropping_it() {
        for p in [
            "/private/../",
            "/../",
            "/a/b/../../",
            "/a/..",
            "..",
            "/a/../b/",
        ] {
            assert_eq!(to_relative(Path::new("."), p), None, "{p}");
        }
        assert_eq!(to_relative(Path::new("."), "/a\0b/"), None);
    }

    #[test]
    fn to_relative_normalises_harmless_paths() {
        let rel = |p| to_relative(Path::new("."), p);
        assert_eq!(rel("/"), Some(PathBuf::from(".")));
        assert_eq!(rel("//pub//sub/./"), Some(PathBuf::from("./pub/sub")));
        assert_eq!(rel("/pub/..."), Some(PathBuf::from("./pub/...")));
    }

    fn location(path: &str, query: Option<&str>) -> String {
        let rel = to_relative(Path::new("."), path).unwrap();
        let response = redirect_to_directory(&rel, query);
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn redirect_target_is_always_a_same_origin_path() {
        assert_eq!(location("/pub", None), "/pub/");
        assert_eq!(location("/a b", Some("x=1")), "/a%20b/?x=1");
        // scheme-relative for a browser if copied verbatim into Location
        assert_eq!(location("//evil.example", None), "/evil.example/");
        assert_eq!(location("/\\evil.example", None), "/%5Cevil.example/");
        assert_eq!(location("/\t/evil.example", None), "/%09/evil.example/");
        // used to panic in Redirect::permanent ("URI isn't a valid header value")
        assert_eq!(location("/x\ny", None), "/x%0Ay/");
        assert_eq!(
            location("/x\r\nSet-Cookie: a=b", None),
            "/x%0D%0ASet-Cookie%3A%20a%3Db/"
        );
    }

    #[test]
    fn errors_have_real_status_codes() {
        let status = |source: io::Error| YadexError::NotFound { source }.into_response().status();
        assert_eq!(
            status(io::ErrorKind::NotFound.into()),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(io::ErrorKind::PermissionDenied.into()),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            invalid_path().into_response().status(),
            StatusCode::NOT_FOUND
        );
        // "too many open files" is not "this directory does not exist"
        let emfile = io::Error::from_raw_os_error(rustix::io::Errno::MFILE.raw_os_error());
        assert_eq!(status(emfile), StatusCode::SERVICE_UNAVAILABLE);
    }
}
