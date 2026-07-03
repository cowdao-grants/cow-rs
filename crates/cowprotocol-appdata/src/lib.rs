//! CoW Protocol app-data document and digest helpers.

#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod app_data;

pub use self::app_data::{
    APP_DATA_SIZE_LIMIT, AppDataCid, AppDataCidError, AppDataDoc, AppDataError, AppDataFlashloan,
    AppDataHash, AppDataHooks, AppDataMetadata, AppDataOrderClass, AppDataPartnerFee, AppDataQuote,
    AppDataReferrer, AppDataReplacedOrder, AppDataUtm, AppDataWrapperCall, COW_RS_APP_CODE,
    COW_RS_WASM_APP_CODE, EMPTY_APP_DATA_HASH, EMPTY_APP_DATA_JSON, FeePolicy, Hook,
    LATEST_APP_DATA_VERSION, MAX_CID_STR_LEN, PreparedAppData, app_data_cid,
    app_data_hash_from_cid, parse_app_data_cid,
};
