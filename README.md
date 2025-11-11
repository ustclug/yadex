# yadex

Yet Another inDEX page.

Designed to work with other servers like nginx -- yadex does not serve files, instead it only generates index pages for directories.

## Endpoints

### Template-rendered index page

Enabled with `template_index` config option in `[service]` (default: true). You need to set `index_file` in `[template]` config section (relative to config dir).

Template example: [etc/index.html](etc/index.html)

### JSON API

Enabled with `json_api` config option in `[service]` (default: false). The path is `/api/files`, and shall be called with a POST request with a JSON body:

```json
{
  "path": "/path/to/directory"
}
```

Example response (note: entries are not sorted):

```json
{
  "entries": [
    {
      "name": "hooks",
      "is_dir": true,
      "size": 556,
      "href": "/.git/hooks/",
      "datetime": 1762543427
    },
    {
      "name": "info",
      "is_dir": true,
      "size": 14,
      "href": "/.git/info/",
      "datetime": 1762543427
    },
    {
      "name": "description",
      "is_dir": false,
      "size": 73,
      "href": "/.git/description",
      "datetime": 1762543427
    },
    {
      "name": "objects",
      "is_dir": true,
      "size": 408,
      "href": "/.git/objects/",
      "datetime": 1762890787
    },
    {
      "name": "refs",
      "is_dir": true,
      "size": 32,
      "href": "/.git/refs/",
      "datetime": 1762543429
    },
    {
      "name": "packed-refs",
      "is_dir": false,
      "size": 216,
      "href": "/.git/packed-refs",
      "datetime": 1762543429
    },
    {
      "name": "logs",
      "is_dir": true,
      "size": 16,
      "href": "/.git/logs/",
      "datetime": 1762543429
    },
    {
      "name": "HEAD",
      "is_dir": false,
      "size": 21,
      "href": "/.git/HEAD",
      "datetime": 1762543429
    },
    {
      "name": "COMMIT_EDITMSG",
      "is_dir": false,
      "size": 40,
      "href": "/.git/COMMIT_EDITMSG",
      "datetime": 1762890785
    },
    {
      "name": "FETCH_HEAD",
      "is_dir": false,
      "size": 82,
      "href": "/.git/FETCH_HEAD",
      "datetime": 1762890787
    },
    {
      "name": "config",
      "is_dir": false,
      "size": 284,
      "href": "/.git/config",
      "datetime": 1762546352
    },
    {
      "name": "index",
      "is_dir": false,
      "size": 1635,
      "href": "/.git/index",
      "datetime": 1762890785
    },
    {
      "name": "ORIG_HEAD",
      "is_dir": false,
      "size": 41,
      "href": "/.git/ORIG_HEAD",
      "datetime": 1762890787
    }
  ],
  "maybe_truncated": false
}
```
