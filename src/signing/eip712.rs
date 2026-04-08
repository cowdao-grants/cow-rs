use crate::{
    config::SdkConfig,
    domain::order::{BuyTokenDestination, Order, OrderKind, SellTokenSource},
    error::{CowError, Result},
};
use ethers_core::{
    abi::{encode, Token},
    types::{Address, H256, U256},
    utils::keccak256,
};
use std::str::FromStr;

const DOMAIN_TYPE: &str =
    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";
const ORDER_TYPE: &str = "Order(address sellToken,address buyToken,address receiver,uint256 sellAmount,uint256 buyAmount,uint32 validTo,bytes32 appData,uint256 feeAmount,string kind,bool partiallyFillable,string sellTokenBalance,string buyTokenBalance)";
const DOMAIN_NAME: &str = "Gnosis Protocol";
const DOMAIN_VERSION: &str = "v2";
const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

pub fn order_signing_hash(order: &Order, config: &SdkConfig) -> Result<[u8; 32]> {
    let domain_separator = domain_separator(config)?;
    let order_hash = hash_order(order)?;
    Ok(keccak256(
        [&[0x19, 0x01][..], &domain_separator[..], &order_hash[..]].concat(),
    ))
}

fn domain_separator(config: &SdkConfig) -> Result<[u8; 32]> {
    let verifying_contract = parse_address(config.settlement_contract())?;
    let encoded = encode(&[
        Token::FixedBytes(keccak256(DOMAIN_TYPE).to_vec()),
        Token::FixedBytes(keccak256(DOMAIN_NAME).to_vec()),
        Token::FixedBytes(keccak256(DOMAIN_VERSION).to_vec()),
        Token::Uint(U256::from(config.chain_id.as_u64())),
        Token::Address(verifying_contract),
    ]);

    Ok(keccak256(encoded))
}

fn hash_order(order: &Order) -> Result<[u8; 32]> {
    let sell_token = parse_address(&order.sell_token)?;
    let buy_token = parse_address(&order.buy_token)?;
    let receiver = parse_address(order.receiver.as_deref().unwrap_or(ZERO_ADDRESS))?;
    let app_data = parse_h256(&order.app_data)?;
    let sell_amount = parse_u256(&order.sell_amount, "sellAmount")?;
    let buy_amount = parse_u256(&order.buy_amount, "buyAmount")?;
    let fee_amount = parse_u256(&order.fee_amount, "feeAmount")?;

    let encoded = encode(&[
        Token::FixedBytes(keccak256(ORDER_TYPE).to_vec()),
        Token::Address(sell_token),
        Token::Address(buy_token),
        Token::Address(receiver),
        Token::Uint(sell_amount),
        Token::Uint(buy_amount),
        Token::Uint(U256::from(order.valid_to)),
        Token::FixedBytes(app_data.0.to_vec()),
        Token::Uint(fee_amount),
        Token::FixedBytes(keccak256(kind_as_str(&order.kind)).to_vec()),
        Token::Bool(order.partially_fillable),
        Token::FixedBytes(keccak256(sell_token_balance_as_str(&order.sell_token_balance)).to_vec()),
        Token::FixedBytes(keccak256(buy_token_balance_as_str(&order.buy_token_balance)).to_vec()),
    ]);

    Ok(keccak256(encoded))
}

fn parse_address(value: &str) -> Result<Address> {
    Address::from_str(value).map_err(|_| CowError::InvalidAddress(value.to_owned()))
}

fn parse_h256(value: &str) -> Result<H256> {
    H256::from_str(value).map_err(|_| CowError::InvalidBytes32(value.to_owned()))
}

fn parse_u256(value: &str, field: &'static str) -> Result<U256> {
    U256::from_dec_str(value).map_err(|_| CowError::InvalidAmount {
        field,
        value: value.to_owned(),
    })
}

fn kind_as_str(kind: &OrderKind) -> &'static str {
    match kind {
        OrderKind::Buy => "buy",
        OrderKind::Sell => "sell",
    }
}

fn sell_token_balance_as_str(balance: &SellTokenSource) -> &'static str {
    match balance {
        SellTokenSource::Erc20 => "erc20",
        SellTokenSource::External => "external",
        SellTokenSource::Internal => "internal",
    }
}

fn buy_token_balance_as_str(balance: &BuyTokenDestination) -> &'static str {
    match balance {
        BuyTokenDestination::Erc20 => "erc20",
        BuyTokenDestination::Internal => "internal",
    }
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
    fn produces_deterministic_hash() {
        let config = SdkConfig::new("cow-rs", ChainId::Sepolia, Environment::Staging);
        let first = order_signing_hash(&sample_order(), &config).unwrap();
        let second = order_signing_hash(&sample_order(), &config).unwrap();

        assert_eq!(first, second);
        assert_ne!(first, [0u8; 32]);
    }
}
