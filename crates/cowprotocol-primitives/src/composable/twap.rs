//! Canonical `TWAP` handler `staticInput` payload.
//!
//! One of the handler-specific `staticInput` modules promised by the
//! [`composable`](super) module doc. [`TwapData`] is the user-facing
//! parameter set; it validates against the on-chain `TWAPOrder.validate`
//! checks and lowers to the canonical [`TwapStaticInput`] the handler
//! reads, wrapped into a [`ConditionalOrderParams`] triple keyed by the
//! [`TWAP_HANDLER`] address. The canonical Solidity sources live in
//! [`cowprotocol/composable-cow`][cc].
//!
//! [cc]: https://github.com/cowprotocol/composable-cow

use alloy_primitives::{Address, B256, Bytes, U256, address, keccak256};
use alloy_sol_types::{SolValue, sol};

use super::ConditionalOrderParams;

sol! {
    /// Per-part static input for the canonical `TWAP` handler.
    ///
    /// Mirrors `TWAPOrder.Data` in
    /// [`composable-cow/src/types/twap/libraries/TWAPOrder.sol`][src].
    /// All ten fields are static, so ABI-encoding the struct produces
    /// `10 * 32 = 320` bytes that match
    /// `eth_abi.encode(["(address,address,address,uint256,uint256,uint256,uint256,uint256,uint256,bytes32)"], [[...]])`
    /// byte for byte (cow-py's `Twap.encode_static_input`).
    ///
    /// [src]: https://github.com/cowprotocol/composable-cow/blob/main/src/types/twap/libraries/TWAPOrder.sol
    #[derive(Debug, Eq, Hash, PartialEq)]
    struct TwapStaticInput {
        address sellToken;
        address buyToken;
        address receiver;
        uint256 partSellAmount;
        uint256 minPartLimit;
        uint256 t0;
        uint256 n;
        uint256 t;
        uint256 span;
        bytes32 appData;
    }
}

/// Canonical CREATE2 address of the `TWAP` handler, identical on every
/// chain where the ComposableCoW suite is deployed.
///
/// Source: `composable-cow/networks.json` and cow-py's
/// `TWAP_ADDRESS` (`cowdao_cowpy/composable/order_types/twap.py:24`).
pub const TWAP_HANDLER: Address = address!("0x6cF1e9cA41f7611dEf408122793c358a3d11E5a5");

/// When a TWAP becomes active.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TwapStart {
    /// Start at the block timestamp the registration is mined in. The
    /// watch tower reads the timestamp through the
    /// [`CURRENT_BLOCK_TIMESTAMP_FACTORY`](super::CURRENT_BLOCK_TIMESTAMP_FACTORY).
    AtMiningTime,
    /// Start at a specific Unix timestamp (seconds). Must be in
    /// `1..u32::MAX`: `t0 = 0` is the on-chain sentinel for
    /// [`TwapStart::AtMiningTime`] and `u32::MAX` fails the handler's
    /// `t0 < type(uint32).max` guard, so [`TwapData::static_input`]
    /// rejects both with [`TwapError::InvalidEpoch`].
    AtEpoch(u32),
}

/// How long each TWAP part is fillable for, within its slot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TwapDuration {
    /// Part is fillable for the full slot (`span = 0`).
    Auto,
    /// Part is fillable for the first `seconds` of its slot only; must
    /// be `<= time_between_parts` (the TWAP handler reverts otherwise).
    LimitDuration(u32),
}

/// User-facing TWAP order parameters. Mirrors `TwapData` in
/// `cowdao_cowpy/composable/order_types/twap.py`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TwapData {
    /// Token the owner sells.
    pub sell_token: Address,
    /// Token the owner buys.
    pub buy_token: Address,
    /// Receiver of the buy token. Set to the owner address for self-fills.
    pub receiver: Address,
    /// Total sell amount across every part, in atomic units. The handler
    /// is given `sell_amount / number_of_parts` per part.
    pub sell_amount: U256,
    /// Total minimum buy amount across every part, in atomic units. The
    /// handler enforces `buy_amount / number_of_parts` per part.
    pub buy_amount: U256,
    /// Seconds between part start timestamps. Must be > 0 and
    /// <= 365 days (the handler reverts outside that range).
    pub time_between_parts: u32,
    /// Number of parts. Must be > 1 (the handler reverts otherwise).
    pub number_of_parts: u32,
    /// When the TWAP becomes active.
    pub start: TwapStart,
    /// How long each part is fillable for.
    pub duration: TwapDuration,
    /// 32-byte app-data digest applied to every discrete part.
    pub app_data: B256,
}

/// Failures returned by [`TwapData::static_input`] /
/// [`TwapData::encode_static_input`] / [`TwapData::leaf_id`].
///
/// Mirrors the `OrderNotValid(string)` reverts raised by `TWAPOrder.validate`
/// in composable-cow; client-side validation lets callers reject malformed
/// TWAPs without paying the on-chain revert cost.
#[derive(Clone, Copy, Debug, thiserror::Error, Eq, PartialEq)]
pub enum TwapError {
    /// `sell_token == buy_token`.
    #[error("TWAP sell_token equals buy_token")]
    SameToken,
    /// One of the tokens is the zero address.
    #[error("TWAP token is the zero address")]
    InvalidToken,
    /// `sell_amount == 0` or doesn't divide cleanly into a non-zero
    /// per-part amount.
    #[error("TWAP sell amount is zero or does not divide cleanly across number_of_parts")]
    InvalidSellAmount,
    /// `buy_amount == 0` or doesn't divide cleanly into a non-zero
    /// per-part amount.
    #[error("TWAP buy amount is zero or does not divide cleanly across number_of_parts")]
    InvalidBuyAmount,
    /// `number_of_parts <= 1`.
    #[error("TWAP number_of_parts must be > 1")]
    InvalidNumParts,
    /// `time_between_parts == 0` or `> 365 days`.
    #[error("TWAP time_between_parts must be > 0 and <= 365 days")]
    InvalidFrequency,
    /// `LimitDuration(span)` with `span > time_between_parts`.
    #[error("TWAP LimitDuration span must be <= time_between_parts")]
    InvalidSpan,
    /// `TwapStart::AtEpoch` with a reserved value. `0` collides with the
    /// `t0 = 0` sentinel the handler uses for `AtMiningTime`; `u32::MAX`
    /// is rejected on-chain (`TWAPOrder.validate` requires
    /// `t0 < type(uint32).max`). Use a Unix timestamp in `1..u32::MAX` or
    /// switch to `TwapStart::AtMiningTime`.
    #[error("TWAP AtEpoch must be in 1..u32::MAX (0 and u32::MAX are reserved)")]
    InvalidEpoch,
}

const TWAP_MAX_FREQUENCY_SECONDS: u32 = 365 * 24 * 60 * 60;

impl TwapData {
    /// Validate the parameters and lower to the canonical
    /// [`TwapStaticInput`] the handler sees, splitting the total amounts
    /// across `number_of_parts`.
    pub fn static_input(&self) -> Result<TwapStaticInput, TwapError> {
        if self.sell_token == self.buy_token {
            return Err(TwapError::SameToken);
        }
        if self.sell_token == Address::ZERO || self.buy_token == Address::ZERO {
            return Err(TwapError::InvalidToken);
        }
        if self.number_of_parts <= 1 {
            return Err(TwapError::InvalidNumParts);
        }
        if self.time_between_parts == 0 || self.time_between_parts > TWAP_MAX_FREQUENCY_SECONDS {
            return Err(TwapError::InvalidFrequency);
        }
        let span = match self.duration {
            TwapDuration::Auto => 0,
            TwapDuration::LimitDuration(s) => {
                if s > self.time_between_parts {
                    return Err(TwapError::InvalidSpan);
                }
                s
            }
        };
        let t0 = match self.start {
            TwapStart::AtMiningTime => 0,
            // `0` collides with the AtMiningTime sentinel; `u32::MAX` is
            // rejected by the on-chain `t0 < type(uint32).max` guard.
            TwapStart::AtEpoch(0 | u32::MAX) => return Err(TwapError::InvalidEpoch),
            TwapStart::AtEpoch(s) => s,
        };
        let n = U256::from(self.number_of_parts);
        // Floor-division would silently discard the remainder and let
        // the aggregate min-buy / max-sell drift below user intent
        // (e.g. `buy_amount = 101, n = 10` produces `minPartLimit = 10`,
        // aggregate floor 100). Reject indivisible totals so the
        // signed leaf matches the user's stated amounts exactly.
        let part_sell_amount = self.sell_amount / n;
        let min_part_limit = self.buy_amount / n;
        if part_sell_amount.is_zero() || self.sell_amount % n != U256::ZERO {
            return Err(TwapError::InvalidSellAmount);
        }
        if min_part_limit.is_zero() || self.buy_amount % n != U256::ZERO {
            return Err(TwapError::InvalidBuyAmount);
        }
        Ok(TwapStaticInput {
            sellToken: self.sell_token,
            buyToken: self.buy_token,
            receiver: self.receiver,
            partSellAmount: part_sell_amount,
            minPartLimit: min_part_limit,
            t0: U256::from(t0),
            n,
            t: U256::from(self.time_between_parts),
            span: U256::from(span),
            appData: self.app_data,
        })
    }

    /// Validate the parameters and ABI-encode the canonical static input.
    /// Locked byte-exact against cow-py via the
    /// `twap_leaf_id_matches_cow_py_vector` test.
    pub fn encode_static_input(&self) -> Result<Vec<u8>, TwapError> {
        Ok(self.static_input()?.abi_encode())
    }

    /// Wrap into a [`ConditionalOrderParams`] with the canonical
    /// [`TWAP_HANDLER`] address. Allocates owned bytes from a `&self`
    /// view, hence the `to_` prefix per Rust API conventions.
    pub fn to_params(&self, salt: B256) -> Result<ConditionalOrderParams, TwapError> {
        Ok(ConditionalOrderParams {
            handler: TWAP_HANDLER,
            salt,
            staticInput: Bytes::from(self.static_input()?.abi_encode()),
        })
    }

    /// Compute the canonical leaf id
    /// `keccak256(abi.encode(ConditionalOrderParams))` for this TWAP and
    /// salt. Matches `ConditionalOrder.id` in cow-py.
    pub fn leaf_id(&self, salt: B256) -> Result<B256, TwapError> {
        Ok(keccak256(self.to_params(salt)?.abi_encode()))
    }

    /// Start a [`TwapDataBuilder`] from the three addresses every TWAP
    /// needs. Set the remaining parameters with the builder's `with_*`
    /// methods, then call [`TwapDataBuilder::build`].
    pub const fn builder(
        sell_token: Address,
        buy_token: Address,
        receiver: Address,
    ) -> TwapDataBuilder {
        TwapDataBuilder {
            sell_token,
            buy_token,
            receiver,
            sell_amount: U256::ZERO,
            buy_amount: U256::ZERO,
            time_between_parts: 0,
            number_of_parts: 0,
            start: TwapStart::AtMiningTime,
            duration: TwapDuration::Auto,
            app_data: B256::ZERO,
        }
    }
}

/// Fluent builder for [`TwapData`].
///
/// Obtain one from [`TwapData::builder`] with the sell, buy and receiver
/// addresses, set the remaining parameters with the `with_*` methods,
/// then call [`TwapDataBuilder::build`] to validate and produce a
/// [`TwapData`]. The same checks as [`TwapData::static_input`] run once
/// at build time, so a malformed parameter set surfaces its
/// [`TwapError`] before the order is used.
///
/// Unset numeric fields default to `0` and therefore fail validation
/// until set; `start` defaults to [`TwapStart::AtMiningTime`],
/// `duration` to [`TwapDuration::Auto`], and `app_data` to the zero
/// digest.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TwapDataBuilder {
    sell_token: Address,
    buy_token: Address,
    receiver: Address,
    sell_amount: U256,
    buy_amount: U256,
    time_between_parts: u32,
    number_of_parts: u32,
    start: TwapStart,
    duration: TwapDuration,
    app_data: B256,
}

impl TwapDataBuilder {
    /// Set the total sell amount across every part, in atomic units.
    #[must_use]
    pub const fn with_sell_amount(mut self, sell_amount: U256) -> Self {
        self.sell_amount = sell_amount;
        self
    }

    /// Set the total minimum buy amount across every part, in atomic
    /// units.
    #[must_use]
    pub const fn with_buy_amount(mut self, buy_amount: U256) -> Self {
        self.buy_amount = buy_amount;
        self
    }

    /// Set the seconds between part start timestamps.
    #[must_use]
    pub const fn with_time_between_parts(mut self, seconds: u32) -> Self {
        self.time_between_parts = seconds;
        self
    }

    /// Set the number of parts the order is split into.
    #[must_use]
    pub const fn with_number_of_parts(mut self, number_of_parts: u32) -> Self {
        self.number_of_parts = number_of_parts;
        self
    }

    /// Set when the TWAP becomes active.
    #[must_use]
    pub const fn with_start(mut self, start: TwapStart) -> Self {
        self.start = start;
        self
    }

    /// Set how long each part is fillable for within its slot.
    #[must_use]
    pub const fn with_duration(mut self, duration: TwapDuration) -> Self {
        self.duration = duration;
        self
    }

    /// Set the 32-byte app-data digest applied to every part.
    #[must_use]
    pub const fn with_app_data(mut self, app_data: B256) -> Self {
        self.app_data = app_data;
        self
    }

    /// Validate the assembled parameters and produce a [`TwapData`].
    ///
    /// Runs the [`TwapData::static_input`] checks; returns the assembled
    /// [`TwapData`] on success or the first [`TwapError`] otherwise.
    pub fn build(self) -> Result<TwapData, TwapError> {
        let data = TwapData {
            sell_token: self.sell_token,
            buy_token: self.buy_token,
            receiver: self.receiver,
            sell_amount: self.sell_amount,
            buy_amount: self.buy_amount,
            time_between_parts: self.time_between_parts,
            number_of_parts: self.number_of_parts,
            start: self.start,
            duration: self.duration,
            app_data: self.app_data,
        };
        data.static_input()?;
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{B256, hex};

    use super::*;

    /// Locks the TWAP leaf-id derivation against cow-py's canonical
    /// vector at `cow-py::tests/composable/order_types/test_twap.py:23`.
    /// If `TwapStaticInput::abi_encode` ever drifts from
    /// `eth_abi.encode("(address,...,bytes32)", ...)`, the id mismatches
    /// and the watch tower can't find the order.
    #[test]
    fn twap_leaf_id_matches_cow_py_vector() {
        let twap = TwapData {
            sell_token: address!("6810e776880C02933D47DB1b9fc05908e5386b96"),
            buy_token: address!("DAE5F1590db13E3B40423B5b5c5fbf175515910b"),
            receiver: address!("DeaDbeefdEAdbeefdEadbEEFdeadbeEFdEaDbeeF"),
            sell_amount: U256::from(1_000_000_000_000_000_000_u128),
            buy_amount: U256::from(1_000_000_000_000_000_000_u128),
            time_between_parts: 3600,
            number_of_parts: 10,
            start: TwapStart::AtMiningTime,
            duration: TwapDuration::Auto,
            app_data: B256::from(hex!(
                "d51f28edffcaaa76be4a22f6375ad289272c037f3cc072345676e88d92ced8b5"
            )),
        };
        let salt = B256::from(hex!(
            "d98a87ed4e45bfeae3f779e1ac09ceacdfb57da214c7fffa6434aeb969f396c0"
        ));
        let id = twap.leaf_id(salt).unwrap();
        assert_eq!(
            id.0,
            hex!("d8a6889486a47d8ca8f4189f11573b39dbc04f605719ebf4050e44ae53c1bedf"),
        );

        // Second cow-py vector with a different salt; same TwapData
        // produces a different id, confirming the salt path.
        let salt2 = B256::from(hex!(
            "d98a87ed4e45bfeae3f779e1ac09ceacdfb57da214c7fffa6434aeb969f396c1"
        ));
        let id2 = twap.leaf_id(salt2).unwrap();
        assert_eq!(
            id2.0,
            hex!("8ddb7e8e1cd6a06d5bb6f91af21a2b26a433a5d8402ccddb00a72e4006c46994"),
        );
    }

    /// Validation must reject parameter combinations the on-chain
    /// `TWAPOrder.validate` reverts on.
    #[test]
    fn twap_validation_rejects_invalid_parameters() {
        let base = TwapData {
            sell_token: address!("6810e776880C02933D47DB1b9fc05908e5386b96"),
            buy_token: address!("DAE5F1590db13E3B40423B5b5c5fbf175515910b"),
            receiver: address!("DeaDbeefdEAdbeefdEadbEEFdeadbeEFdEaDbeeF"),
            sell_amount: U256::from(1_000_000_000_000_000_000_u128),
            buy_amount: U256::from(1_000_000_000_000_000_000_u128),
            time_between_parts: 3600,
            number_of_parts: 10,
            start: TwapStart::AtMiningTime,
            duration: TwapDuration::Auto,
            app_data: B256::ZERO,
        };
        assert!(base.static_input().is_ok());

        let mut same_token = base;
        same_token.buy_token = base.sell_token;
        assert_eq!(same_token.static_input().unwrap_err(), TwapError::SameToken);

        let mut zero_sell = base;
        zero_sell.sell_token = alloy_primitives::Address::ZERO;
        assert_eq!(
            zero_sell.static_input().unwrap_err(),
            TwapError::InvalidToken
        );

        let mut one_part = base;
        one_part.number_of_parts = 1;
        assert_eq!(
            one_part.static_input().unwrap_err(),
            TwapError::InvalidNumParts
        );

        let mut zero_freq = base;
        zero_freq.time_between_parts = 0;
        assert_eq!(
            zero_freq.static_input().unwrap_err(),
            TwapError::InvalidFrequency
        );

        let mut excess_span = base;
        excess_span.duration = TwapDuration::LimitDuration(7200);
        assert_eq!(
            excess_span.static_input().unwrap_err(),
            TwapError::InvalidSpan
        );

        let mut tiny_sell = base;
        tiny_sell.sell_amount = U256::from(5_u64); // 5 / 10 = 0
        assert_eq!(
            tiny_sell.static_input().unwrap_err(),
            TwapError::InvalidSellAmount
        );

        // Indivisible totals must be rejected so the signed leaf
        // matches user intent: 101 / 10 floors to a per-part 10 and
        // an aggregate min-buy of 100, one unit below what the user
        // asked for.
        let mut indivisible_sell = base;
        indivisible_sell.sell_amount = U256::from(101_u64);
        assert_eq!(
            indivisible_sell.static_input().unwrap_err(),
            TwapError::InvalidSellAmount
        );
        let mut indivisible_buy = base;
        indivisible_buy.buy_amount = U256::from(101_u64);
        assert_eq!(
            indivisible_buy.static_input().unwrap_err(),
            TwapError::InvalidBuyAmount
        );
    }

    /// `TwapStart::AtEpoch(0)` collides with the `t0 = 0` sentinel the
    /// on-chain handler reserves for `AtMiningTime`. Reject it so the
    /// leaf id and registered order match caller intent.
    #[test]
    fn twap_at_epoch_zero_rejected() {
        let twap = TwapData {
            sell_token: address!("6810e776880C02933D47DB1b9fc05908e5386b96"),
            buy_token: address!("DAE5F1590db13E3B40423B5b5c5fbf175515910b"),
            receiver: address!("DeaDbeefdEAdbeefdEadbEEFdeadbeEFdEaDbeeF"),
            sell_amount: U256::from(1_000_000_000_000_000_000_u128),
            buy_amount: U256::from(1_000_000_000_000_000_000_u128),
            time_between_parts: 3600,
            number_of_parts: 10,
            start: TwapStart::AtEpoch(0),
            duration: TwapDuration::Auto,
            app_data: B256::ZERO,
        };
        assert_eq!(twap.static_input().unwrap_err(), TwapError::InvalidEpoch);

        // `u32::MAX` fails the on-chain `t0 < type(uint32).max` guard.
        let mut at_max = twap;
        at_max.start = TwapStart::AtEpoch(u32::MAX);
        assert_eq!(at_max.static_input().unwrap_err(), TwapError::InvalidEpoch);

        // A timestamp just under the cap is accepted.
        let mut just_under = twap;
        just_under.start = TwapStart::AtEpoch(u32::MAX - 1);
        assert!(just_under.static_input().is_ok());
    }

    /// `AtMiningTime` legitimately encodes to `t0 = 0`: this lock-in
    /// guards against anyone "fixing" the sentinel branch to also
    /// reject zero, which would break the cabinet-anchored flow.
    #[test]
    fn twap_at_mining_time_still_encodes_t0_zero() {
        let twap = TwapData {
            sell_token: address!("6810e776880C02933D47DB1b9fc05908e5386b96"),
            buy_token: address!("DAE5F1590db13E3B40423B5b5c5fbf175515910b"),
            receiver: address!("DeaDbeefdEAdbeefdEadbEEFdeadbeEFdEaDbeeF"),
            sell_amount: U256::from(1_000_000_000_000_000_000_u128),
            buy_amount: U256::from(1_000_000_000_000_000_000_u128),
            time_between_parts: 3600,
            number_of_parts: 10,
            start: TwapStart::AtMiningTime,
            duration: TwapDuration::Auto,
            app_data: B256::ZERO,
        };
        assert_eq!(twap.static_input().unwrap().t0, U256::ZERO);
    }

    /// The rejection is exact to the zero sentinel: any non-zero
    /// `AtEpoch(s)` round-trips into `t0 = s`.
    #[test]
    fn twap_at_epoch_one_round_trips() {
        let twap = TwapData {
            sell_token: address!("6810e776880C02933D47DB1b9fc05908e5386b96"),
            buy_token: address!("DAE5F1590db13E3B40423B5b5c5fbf175515910b"),
            receiver: address!("DeaDbeefdEAdbeefdEadbEEFdeadbeEFdEaDbeeF"),
            sell_amount: U256::from(1_000_000_000_000_000_000_u128),
            buy_amount: U256::from(1_000_000_000_000_000_000_u128),
            time_between_parts: 3600,
            number_of_parts: 10,
            start: TwapStart::AtEpoch(1),
            duration: TwapDuration::Auto,
            app_data: B256::ZERO,
        };
        assert_eq!(twap.static_input().unwrap().t0, U256::from(1u32));
    }

    /// The builder assembles the same value as a struct literal and
    /// surfaces validation at `build()` time.
    #[test]
    fn builder_matches_struct_literal_and_validates() {
        let sell_token = address!("6810e776880C02933D47DB1b9fc05908e5386b96");
        let buy_token = address!("DAE5F1590db13E3B40423B5b5c5fbf175515910b");
        let receiver = address!("DeaDbeefdEAdbeefdEadbEEFdeadbeEFdEaDbeeF");
        let app_data = B256::from(hex!(
            "d51f28edffcaaa76be4a22f6375ad289272c037f3cc072345676e88d92ced8b5"
        ));
        let built = TwapData::builder(sell_token, buy_token, receiver)
            .with_sell_amount(U256::from(1_000_000_000_000_000_000_u128))
            .with_buy_amount(U256::from(1_000_000_000_000_000_000_u128))
            .with_time_between_parts(3600)
            .with_number_of_parts(10)
            .with_start(TwapStart::AtEpoch(1))
            .with_duration(TwapDuration::LimitDuration(1800))
            .with_app_data(app_data)
            .build()
            .unwrap();
        let expected = TwapData {
            sell_token,
            buy_token,
            receiver,
            sell_amount: U256::from(1_000_000_000_000_000_000_u128),
            buy_amount: U256::from(1_000_000_000_000_000_000_u128),
            time_between_parts: 3600,
            number_of_parts: 10,
            start: TwapStart::AtEpoch(1),
            duration: TwapDuration::LimitDuration(1800),
            app_data,
        };
        assert_eq!(built, expected);

        // Defaults (zero amounts / parts) fail the same checks as
        // `static_input`, so `build()` surfaces the error.
        assert_eq!(
            TwapData::builder(sell_token, buy_token, receiver)
                .build()
                .unwrap_err(),
            TwapError::InvalidNumParts,
        );
    }
}
