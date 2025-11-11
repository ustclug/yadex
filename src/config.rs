use std::{net::IpAddr, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq)]
pub enum Security {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "chroot")]
    Chroot,
    #[serde(rename = "landlock")]
    Landlock,
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub network: NetworkConfig,
    pub template: TemplateConfig,
    pub service: ServiceConfig,
}

#[derive(Serialize, Deserialize)]
pub struct NetworkConfig {
    pub address: IpAddr,
    pub port: u16,
}

#[derive(Serialize, Deserialize)]
pub struct TemplateConfig {
    #[serde(default = "defaults::default_index_file")]
    pub index_file: PathBuf,
}

#[derive(Serialize, Deserialize)]
pub struct ServiceConfig {
    pub limit: u64,
    pub root: PathBuf,
    pub security: Security,
    #[serde(default = "defaults::bool_true")]
    pub template_index: bool,
    #[serde(default = "defaults::bool_false")]
    pub json_api: bool,
}

mod defaults {
    pub fn bool_true() -> bool {
        true
    }

    pub fn bool_false() -> bool {
        false
    }

    pub fn default_index_file() -> std::path::PathBuf {
        "index.html".to_string().into()
    }
}
