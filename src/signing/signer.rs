use crate::{
    config::SdkConfig,
    domain::order::{Order, SigningScheme},
    error::{CowError, Result},
    signing::eip712::order_signing_hash,
};
use ethers_core::types::{Signature, H256};
use ethers_signers::LocalWallet;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SigningResult {
    pub signature: String,
    pub signing_scheme: SigningScheme,
}

pub fn sign_order(
    order: &Order,
    config: &SdkConfig,
    wallet: &LocalWallet,
) -> Result<SigningResult> {
    let digest = order_signing_hash(order, config)?;
    let signature = wallet
        .sign_hash(H256::from(digest))
        .map_err(|err| CowError::Wallet(err.to_string()))?;

    Ok(SigningResult {
        signature: hex_signature(signature),
        signing_scheme: SigningScheme::Eip712,
    })
}

fn hex_signature(signature: Signature) -> String {
    let mut bytes = Vec::with_capacity(65);
    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    signature.r.to_big_endian(&mut r);
    signature.s.to_big_endian(&mut s);
    bytes.extend_from_slice(&r);
    bytes.extend_from_slice(&s);
    bytes.push(signature.v as u8);
    format!("0x{}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{ChainId, Environment},
        domain::order::{BuyTokenDestination, Order, OrderKind, SellTokenSource},
    };

    fn sample_order() -> Order {
        Order {
            sell_token: "0x0000000000000000000000000000000000000001".to_owned(),
            buy_token: "0x0000000000000000000000000000000000000002".to_owned(),
            receiver: None,
            sell_amount: "100".to_owned(),
            buy_amount: "90".to_owned(),
            valid_to: 1_700_000_000,
            app_data: "0xb48d38f93eaa084033fc5970bf96e559c33c4cdc07d889ab00b4d63f9590739d"
                .to_owned(),
            fee_amount: "0".to_owned(),
            kind: OrderKind::Sell,
            partially_fillable: false,
            sell_token_balance: SellTokenSource::Erc20,
            buy_token_balance: BuyTokenDestination::Erc20,
        }
    }

    #[test]
    fn signs_order_with_private_key() {
        let config = SdkConfig::new("cow-rs", ChainId::Sepolia, Environment::Staging);
        let wallet: LocalWallet =
            "0x59c6995e998f97a5a0044966f0945382dbf30dd7ec4f9b89e0c5f6f9d1c1f98a"
                .parse()
                .unwrap();

        let result = sign_order(&sample_order(), &config, &wallet).unwrap();

        assert_eq!(result.signing_scheme, SigningScheme::Eip712);
        assert!(result.signature.starts_with("0x"));
        assert_eq!(result.signature.len(), 132);
    }
}
