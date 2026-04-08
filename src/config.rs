use crate::error::{CowError, Result};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr, time::Duration};
use url::Url;

const PROD_SETTLEMENT_CONTRACT: &str = "0x9008D19f58AAbD9eD0D60971565AA8510560ab41";
const STAGING_SETTLEMENT_CONTRACT: &str = "0xf553d092b50bdcbddeD1A99aF2cA29FBE5E2CB13";
const PROD_ORDERBOOK_ROOT: &str = "https://api.cow.fi";
const STAGING_ORDERBOOK_ROOT: &str = "https://barn.api.cow.fi";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Prod,
    Staging,
}

impl FromStr for Environment {
    type Err = CowError;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "prod" | "production" => Ok(Self::Prod),
            "staging" | "barn" => Ok(Self::Staging),
            other => Err(CowError::UnsupportedEnvironment(other.to_owned())),
        }
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prod => write!(f, "prod"),
            Self::Staging => write!(f, "staging"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u64)]
pub enum ChainId {
    Mainnet = 1,
    Gnosis = 100,
    Sepolia = 11155111,
}

impl TryFrom<u64> for ChainId {
    type Error = CowError;

    fn try_from(value: u64) -> Result<Self> {
        match value {
            1 => Ok(Self::Mainnet),
            100 => Ok(Self::Gnosis),
            11155111 => Ok(Self::Sepolia),
            other => Err(CowError::UnsupportedChainId(other)),
        }
    }
}

impl ChainId {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Gnosis => "xdai",
            Self::Sepolia => "sepolia",
        }
    }

    pub fn as_u64(self) -> u64 {
        self as u64
    }
}

#[derive(Clone, Debug)]
pub struct SdkConfig {
    pub app_code: String,
    pub chain_id: ChainId,
    pub env: Environment,
    pub timeout: Duration,
    pub base_url_override: Option<Url>,
}

impl SdkConfig {
    pub fn new(app_code: impl Into<String>, chain_id: ChainId, env: Environment) -> Self {
        Self {
            app_code: app_code.into(),
            chain_id,
            env,
            timeout: Duration::from_secs(30),
            base_url_override: None,
        }
    }

    pub fn with_base_url(mut self, base_url: Url) -> Self {
        self.base_url_override = Some(base_url);
        self
    }

    pub fn orderbook_base_url(&self) -> Result<Url> {
        if let Some(url) = &self.base_url_override {
            return Ok(url.clone());
        }

        let root = match self.env {
            Environment::Prod => PROD_ORDERBOOK_ROOT,
            Environment::Staging => STAGING_ORDERBOOK_ROOT,
        };

        Url::parse(&format!("{}/{}/", root, self.chain_id.slug())).map_err(Into::into)
    }

    pub fn settlement_contract(&self) -> &'static str {
        match self.env {
            Environment::Prod => PROD_SETTLEMENT_CONTRACT,
            Environment::Staging => STAGING_SETTLEMENT_CONTRACT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_prod_base_url() {
        let config = SdkConfig::new("cow-rs", ChainId::Gnosis, Environment::Prod);
        assert_eq!(
            config.orderbook_base_url().unwrap().as_str(),
            "https://api.cow.fi/xdai/"
        );
    }

    #[test]
    fn resolves_staging_base_url() {
        let config = SdkConfig::new("cow-rs", ChainId::Sepolia, Environment::Staging);
        assert_eq!(
            config.orderbook_base_url().unwrap().as_str(),
            "https://barn.api.cow.fi/sepolia/"
        );
    }
}
