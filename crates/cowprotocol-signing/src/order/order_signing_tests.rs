use alloy_primitives::{Address, B256, U256};
use alloy_signer_local::PrivateKeySigner;
use hex_literal::hex;

use super::{BuyTokenDestination, OrderData, OrderKind, SellTokenSource};
use crate::{
    app_data::AppDataHash, contracts::GPV2_SETTLEMENT as SETTLEMENT,
    signing_scheme::EcdsaSigningScheme,
};

fn sample_order() -> OrderData {
    OrderData {
        sell_token: Address::from(hex!("0101010101010101010101010101010101010101")),
        buy_token: Address::from(hex!("0202020202020202020202020202020202020202")),
        receiver: Some(Address::from(hex!(
            "0303030303030303030303030303030303030303"
        ))),
        sell_amount: U256::from(0x0246ddf97976680000_u128),
        buy_amount: U256::from(0xb98bc829a6f90000_u128),
        valid_to: 0xffffffff,
        app_data: AppDataHash::default(),
        fee_amount: U256::from(0x0de0b6b3a7640000_u128),
        kind: OrderKind::Sell,
        partially_fillable: false,
        sell_token_balance: SellTokenSource::Erc20,
        buy_token_balance: BuyTokenDestination::Erc20,
    }
}

fn fixed_signer() -> PrivateKeySigner {
    let private_key = B256::from(hex!(
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
    ));
    PrivateKeySigner::from_bytes(&private_key).unwrap()
}

#[test]
fn sign_recover_round_trip_on_order_data() {
    let signer = fixed_signer();
    let owner = signer.address();
    let domain = crate::domain::settlement_domain(1, SETTLEMENT);
    let order = sample_order();

    for scheme in [EcdsaSigningScheme::Eip712, EcdsaSigningScheme::EthSign] {
        let signature = order.sign(scheme, &domain, &signer).unwrap();
        let recovered = order
            .recover_signer(&domain, &signature)
            .unwrap()
            .expect("ECDSA schemes recover an owner");
        assert_eq!(recovered.signer, owner);
        assert_eq!(recovered.message, order.signing_hash(scheme, &domain));

        let ecdsa = order.sign_ecdsa(scheme, &domain, &signer).unwrap();
        assert_eq!(
            order.recover_ecdsa(scheme, &domain, &ecdsa).unwrap().signer,
            owner,
        );
    }
}

#[tokio::test]
async fn sign_async_matches_sync_on_order_data() {
    let signer = fixed_signer();
    let domain = crate::domain::settlement_domain(1, SETTLEMENT);
    let order = sample_order();

    for scheme in [EcdsaSigningScheme::Eip712, EcdsaSigningScheme::EthSign] {
        assert_eq!(
            order.sign(scheme, &domain, &signer).unwrap(),
            order.sign_async(scheme, &domain, &signer).await.unwrap(),
        );
        assert_eq!(
            order.sign_ecdsa(scheme, &domain, &signer).unwrap(),
            order
                .sign_ecdsa_async(scheme, &domain, &signer)
                .await
                .unwrap(),
        );
    }
}
