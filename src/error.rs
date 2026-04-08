use thiserror::Error;

pub type Result<T> = std::result::Result<T, CowError>;

#[derive(Debug, Error)]
pub enum CowError {
    #[error("unsupported chain id: {0}")]
    UnsupportedChainId(u64),
    #[error("unsupported environment: {0}")]
    UnsupportedEnvironment(String),
    #[error("unsupported order kind: {0}")]
    UnsupportedOrderKind(String),
    #[error("invalid address `{0}`")]
    InvalidAddress(String),
    #[error("invalid bytes32 `{0}`")]
    InvalidBytes32(String),
    #[error("invalid amount `{value}` for field `{field}`")]
    InvalidAmount { field: &'static str, value: String },
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("url error: {0}")]
    Url(#[from] url::ParseError),
    #[error("serde json error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("wallet error: {0}")]
    Wallet(String),
    #[error("orderbook api returned {status}: {body}")]
    Api { status: u16, body: String },
}
