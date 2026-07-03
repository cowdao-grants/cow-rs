//! The EthFlow periphery contract, the canonical path for users to sell a
//! chain's native gas token (ETH on mainnet, xDAI on Gnosis Chain, and so on)
//! through CoW Protocol without first wrapping it themselves.
//!
//! A user calls `createOrder(EthFlowOrder.Data)` on the EthFlow contract with
//! `msg.value = sellAmount + feeAmount`. The contract wraps the native token
//! into its ERC-20 counterpart (e.g. WETH) and stands in as an ERC-1271
//! "contract intent" signer for an otherwise standard CoW order with
//! `kind = sell`, `validTo = u32::MAX` and an empty (`0x`) signature payload.
//! Only sells are supported: to buy native ETH, sell to WETH and unwrap
//! client-side.
//!
//! This module exposes the on-chain order struct ([`EthFlowOrder`]), plus a
//! helper to project it into the canonical [`OrderData`] payload that flows
//! through the rest of the SDK. Per-chain EthFlow deployment metadata and
//! `CoWSwapEthFlow` ABI bindings live in `cowprotocol-primitives`.
//!
//! Source: `cowprotocol/ethflowcontract/src/libraries/EthFlowOrder.sol` and
//! cow-docs §7 "ETH-flow".

use {
    crate::{
        app_data::AppDataHash,
        error::{Error, Result},
        order::{BuyTokenDestination, OrderData, OrderKind, SellTokenSource},
    },
    alloy_primitives::{Address, U256},
};

pub use cowprotocol_primitives::{
    ETH_FLOW_PRODUCTION, ETH_FLOW_STAGING, EthFlowDeployment, EthFlowEnvironment,
};

/// The on-chain `EthFlowOrder.Data` struct passed to `createOrder`.
///
/// Mirrors the Solidity tuple in
/// [`EthFlowOrder.sol`](https://github.com/cowprotocol/ethflowcontract/blob/main/src/libraries/EthFlowOrder.sol):
///
/// ```solidity
/// struct Data {
///     IERC20 buyToken;
///     address receiver;
///     uint256 sellAmount;
///     uint256 buyAmount;
///     bytes32 appData;
///     uint256 feeAmount;
///     uint32 validTo;
///     bool partiallyFillable;
///     int64 quoteId;
/// }
/// ```
///
/// The sell-token slot is not part of this struct because EthFlow always
/// sells the chain's wrapped-native token; the caller passes that address
/// in to [`EthFlowOrder::try_into_order_data`].
///
/// `receiver` is a mandatory non-zero address: the EthFlow contract is the
/// EIP-1271 order owner, so the GPv2 "receiver = address(0) means the owner
/// receives the buy token" sentinel would route proceeds to the contract
/// rather than the original native-token seller. `CoWSwapEthFlow.createOrder`
/// mirrors this invariant on-chain by reverting with `ReceiverMustBeSet()`;
/// [`EthFlowOrder::try_into_order_data`] enforces it before any hash is produced.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EthFlowOrder {
    /// Token the user wishes to buy with their native ETH.
    pub buy_token: Address,
    /// Recipient of the buy token. Must be non-zero: the on-chain EthFlow
    /// contract rejects `address(0)` because it is itself the order owner
    /// under GPv2's owner-fallback semantics.
    pub receiver: Address,
    /// Amount of native ETH being sold, in wei. Must equal
    /// `msg.value - fee_amount` when `createOrder` is invoked.
    pub sell_amount: U256,
    /// Minimum amount of `buy_token` the user expects.
    pub buy_amount: U256,
    /// 32-byte digest of the canonical app-data JSON.
    pub app_data: AppDataHash,
    /// Protocol fee, paid in the wrapped-native sell token.
    pub fee_amount: U256,
    /// Order expiry as a unix timestamp in seconds. Note that the
    /// CoW-side order produced by [`EthFlowOrder::try_into_order_data`] always
    /// uses `validTo = u32::MAX`; this field stores the *user-facing*
    /// expiry recorded on the EthFlow contract.
    pub valid_to: u32,
    /// Whether partial fills are allowed.
    pub partially_fillable: bool,
    /// Identifier of the off-chain quote backing this order.
    pub quote_id: i64,
}

impl EthFlowOrder {
    /// Project this EthFlow order into the canonical [`OrderData`] payload
    /// that the settlement contract verifies.
    ///
    /// `wrapped_native_token` is the ERC-20 the EthFlow contract wraps the
    /// native gas token into: WETH on Ethereum mainnet, WXDAI on Gnosis
    /// Chain, and so on. The result is a sell order with `validTo` pinned to
    /// `u32::MAX` (the EthFlow contract enforces the user-facing expiry
    /// separately).
    ///
    /// Signing is implicit: the EthFlow contract is the order owner and
    /// signs via ERC-1271 ([`crate::signing_scheme::SigningScheme::Eip1271`])
    /// with an empty (`0x`) signature payload. The SDK's native-sell
    /// sentinel `0xEeee…EEeE` is *not* used here: it is a quote-time
    /// convenience for `OrderBookApi`; the on-chain order produced by this
    /// helper always sets `sell_token = wrapped_native_token`.
    ///
    /// Fails with [`Error::OrderCreationInvalid`] when `receiver` is the
    /// zero address. The on-chain `CoWSwapEthFlow.createOrder` reverts with
    /// `ReceiverMustBeSet()` in that case because the order owner is the
    /// EthFlow contract; encoding `address(0)` in the signed payload would
    /// route proceeds to the contract instead of the original native-token
    /// seller. We check before producing any `OrderData` so a hostile or
    /// careless caller cannot circulate a hash, UID or wire-form order
    /// derived from the unsafe shape.
    pub fn try_into_order_data(&self, wrapped_native_token: Address) -> Result<OrderData> {
        if self.receiver == Address::ZERO {
            return Err(Error::OrderCreationInvalid {
                field: "receiver",
                reason: "EthFlow orders require a non-zero receiver; the EthFlow contract \
                    is the EIP-1271 order owner, so address(0) would route proceeds to it",
            });
        }
        Ok(OrderData {
            sell_token: wrapped_native_token,
            buy_token: self.buy_token,
            receiver: Some(self.receiver),
            sell_amount: self.sell_amount,
            buy_amount: self.buy_amount,
            valid_to: u32::MAX,
            app_data: self.app_data,
            fee_amount: self.fee_amount,
            kind: OrderKind::Sell,
            partially_fillable: self.partially_fillable,
            sell_token_balance: SellTokenSource::Erc20,
            buy_token_balance: BuyTokenDestination::Erc20,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    /// Canonical WETH address on Ethereum mainnet: the wrapped-native token
    /// the production EthFlow deployment hands over to the settlement
    /// contract when sourced on mainnet.
    const WETH_MAINNET: Address = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");

    /// A non-zero, easy-to-eyeball receiver address reused across tests.
    const SAMPLE_RECEIVER: Address = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");

    /// Pin the EthFlow deployment hex literals so a copy-paste regression
    /// on the constants (e.g. staging overwriting production) breaks the
    /// build instead of silently shipping orders to the wrong contract.
    #[test]
    fn deployment_addresses_match_canonical_hex_literals() {
        assert_eq!(
            ETH_FLOW_PRODUCTION,
            address!("bA3cB449bD2B4ADddBc894D8697F5170800EAdeC")
        );
        assert_eq!(
            ETH_FLOW_STAGING,
            address!("04501b9b1D52e67f6862d157E00D13419D2D6E95")
        );
        assert_ne!(ETH_FLOW_PRODUCTION, ETH_FLOW_STAGING);
    }

    /// `try_into_order_data` should pin `sell_token` to the supplied wrapped-native
    /// token, lift the receiver into `Some(...)`, force `valid_to = u32::MAX`
    /// and `kind = Sell`, and leave the user-supplied amounts untouched.
    #[test]
    fn try_into_order_data_projects_canonical_sell_order() {
        let eth_flow = EthFlowOrder {
            buy_token: address!("6B175474E89094C44Da98b954EedeAC495271d0F"), // DAI
            receiver: SAMPLE_RECEIVER,
            sell_amount: U256::from(1_000_000_000_000_000_000_u128), // 1 ETH
            buy_amount: U256::from(3_500_000_000_000_000_000_000_u128), // 3,500 DAI
            app_data: AppDataHash::from([0xab; 32]),
            fee_amount: U256::from(1_500_000_000_000_000_u128), // 0.0015 ETH
            // The user-facing expiry on EthFlow is unrelated to the
            // settlement `validTo`, which must always be `u32::MAX`.
            valid_to: 1_700_000_000,
            partially_fillable: false,
            quote_id: 42,
        };

        let order = eth_flow.try_into_order_data(WETH_MAINNET).unwrap();

        assert_eq!(order.sell_token, WETH_MAINNET);
        assert_eq!(order.buy_token, eth_flow.buy_token);
        assert_eq!(order.receiver, Some(SAMPLE_RECEIVER));
        assert_eq!(order.sell_amount, eth_flow.sell_amount);
        assert_eq!(order.buy_amount, eth_flow.buy_amount);
        assert_eq!(order.fee_amount, eth_flow.fee_amount);
        assert_eq!(order.app_data, eth_flow.app_data);
        assert_eq!(order.valid_to, u32::MAX);
        assert_eq!(order.kind, OrderKind::Sell);
        assert!(!order.partially_fillable);
        assert_eq!(order.sell_token_balance, SellTokenSource::Erc20);
        assert_eq!(order.buy_token_balance, BuyTokenDestination::Erc20);
    }

    /// Mirror the `ReceiverMustBeSet()` revert on `CoWSwapEthFlow.createOrder`:
    /// projecting an EthFlow order whose receiver is the zero address must
    /// fail before any `OrderData` (and therefore any hash, UID, or wire-form
    /// order) is produced. Otherwise, with the EthFlow contract being the
    /// EIP-1271 owner, GPv2's owner-fallback semantics would silently route
    /// proceeds to the contract rather than the original native-token seller.
    #[test]
    fn try_into_order_data_rejects_zero_receiver() {
        let eth_flow = EthFlowOrder {
            buy_token: address!("6B175474E89094C44Da98b954EedeAC495271d0F"),
            receiver: Address::ZERO,
            sell_amount: U256::from(1_u8),
            buy_amount: U256::from(1_u8),
            app_data: AppDataHash::default(),
            fee_amount: U256::ZERO,
            valid_to: 0,
            partially_fillable: true,
            quote_id: 0,
        };

        let err = eth_flow.try_into_order_data(WETH_MAINNET).unwrap_err();
        match err {
            crate::Error::OrderCreationInvalid { field, .. } => assert_eq!(field, "receiver"),
            other => panic!("expected OrderCreationInvalid, got {other:?}"),
        }
    }

    /// Regression guard: the projected `OrderData.receiver` must hold the
    /// concrete user address (i.e. `Some(receiver)`), so its EIP-712 hash
    /// differs from one a future refactor that drops the receiver back to
    /// `None` would produce. The zero-address fallback is exactly what the
    /// audit finding flagged; failing this test means we have regressed.
    #[test]
    fn try_into_order_data_hash_binds_concrete_receiver() {
        let eth_flow = EthFlowOrder {
            buy_token: address!("6B175474E89094C44Da98b954EedeAC495271d0F"),
            receiver: SAMPLE_RECEIVER,
            sell_amount: U256::from(1_u8),
            buy_amount: U256::from(1_u8),
            app_data: AppDataHash::default(),
            fee_amount: U256::ZERO,
            valid_to: 0,
            partially_fillable: false,
            quote_id: 0,
        };

        let order = eth_flow.try_into_order_data(WETH_MAINNET).unwrap();
        assert_eq!(order.receiver, Some(SAMPLE_RECEIVER));

        let mut owner_fallback = order;
        owner_fallback.receiver = None;
        assert_ne!(order.hash_struct(), owner_fallback.hash_struct());
    }
}
