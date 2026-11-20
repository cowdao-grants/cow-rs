//! Quote-to-signed-order amount projection with full fee composition.
//!
//! Mirrors `@cowprotocol/cow-sdk`'s `getQuoteAmountsAndCosts` (TypeScript)
//! byte-for-byte: every intermediate stage (`beforeAllFees`,
//! `afterProtocolFees`, `afterNetworkCosts`, `afterPartnerFees`,
//! `afterSlippage`) and the final `amountsToSign` derive from the same
//! arithmetic the TS path uses, so callers that mix a partner fee with a
//! protocol fee get the same `buyAmount` the orderbook expects.
//!
//! ## Why this exists
//!
//! The basic `sellAmount + feeAmount` adjustment documented at
//! [api §"Step 3"] is only correct in isolation. When the caller layers
//! a partner fee on top of a quote that also
//! carries a protocol fee, the partner-fee base must be computed against
//! the spot price (`beforeAllFees.buyAmount`), which itself depends on
//! the protocol-fee amount. Skipping the protocol-fee leg
//! understates the partner-fee base and overstates the final
//! `buyAmount`; the orderbook rejects or under-fills the resulting
//! order. [`crate::OrderQuoteResponse::try_to_order_data`] therefore
//! routes every quote-to-`OrderData` projection through [`compute`],
//! with [`OrderCosts::default`] reproducing the basic adjustment
//! exactly. The TypeScript SDK fixed the composed case in
//! [`cow-sdk` #867](https://github.com/cowprotocol/cow-sdk/pull/867);
//! this module is the Rust port (against upstream [cow-sdk @
//! `00c3dbd4`](https://github.com/cowprotocol/cow-sdk/blob/00c3dbd41c086ff9a51d5e5a30648615d4c66d0d/packages/order-book/src/quoteAmountsAndCosts/getQuoteAmountsAndCosts.ts),
//! pinned in `parity/source-lock.toml`). Its conformance test asserts
//! the same `1_970_090_045_022_511_257` / `1_970_100_000_000_000_000`
//! divergence the TS test locks.
//!
//! ## Order-of-operations
//!
//! ```text
//! beforeAllFees
//!   └─ afterProtocolFees   (protocol fee deducted from buy / added to sell)
//!        └─ afterNetworkCosts   (network costs already baked in by the API)
//!             └─ afterPartnerFees   (partner-fee base = beforeAllFees)
//!                  └─ afterSlippage    (slippage on the non-fixed side)
//! ```
//!
//! ## Fail-closed arithmetic
//!
//! Every intermediate uses checked `U256` arithmetic and returns
//! [`Error::QuoteFeeMathOverflow`] on overflow or underflow rather than
//! saturating. The TypeScript reference relies on unbounded `BigInt`,
//! so it never silently clamps; the port must reject pathological
//! orderbook inputs (e.g. `sellAmount = U256::MAX`, `slippageBps >
//! 10_000`, `protocolFeeBps >= 100%`) instead of folding a saturated
//! amount into the signed [`crate::OrderData`].
//!
//! [api §"Step 3"]: https://docs.cow.fi/cow-protocol/howto/integrate/api#step-3-compute-the-amounts-to-sign

use core::fmt;
use core::str::FromStr;

use alloy_primitives::U256;

use crate::{Error, error::Result, order::OrderKind};

/// Bps denominator (10_000 = 100%).
const ONE_HUNDRED_BPS: u64 = 10_000;
/// Scale applied to `protocol_fee_bps` so the on-wire decimal string
/// (e.g. `"0.3"`) round-trips through integer arithmetic with five
/// fractional digits of precision, matching the TS SDK's
/// `Math.round(protocolFeeBps * 100_000)`.
const HUNDRED_THOUSANDS: u64 = 100_000;
/// Combined denominator for the protocol-fee `mul_div`: bps are reported
/// against `10_000`, then scaled by `100_000` for fractional precision.
/// Evaluated at compile time, so const evaluation rejects any overflow
/// rather than a runtime `expect` having to.
const SCALE: U256 = U256::from_limbs([(ONE_HUNDRED_BPS * HUNDRED_THOUSANDS), 0, 0, 0]);

/// Default slippage tolerance, in basis points, that
/// [`OrderBookApi::quote_builder`] seeds into [`OrderCosts`]. This is
/// the single place the protocol-recommended `50` is written;
/// [`OrderCosts::default`] is deliberately neutral (zero slippage) so
/// the type itself never injects an adjustment.
///
/// [`OrderBookApi::quote_builder`]: crate::OrderBookApi
pub const DEFAULT_SLIPPAGE_BPS: u32 = 50;

/// Protocol fee in basis points, scaled by `100_000` so the on-wire
/// decimal string (e.g. `"0.3"`) round-trips through integer
/// arithmetic with five fractional digits of precision, matching the
/// TS SDK's `Math.round(protocolFeeBps * 100_000)`.
///
/// The scale is part of the value's meaning, so this is an
/// invariant-enforcing newtype rather than a type alias: a raw `u64`
/// could be confused with an unscaled bps count. Parse the orderbook's
/// decimal string with [`FromStr`]; [`fmt::Display`] renders the wire
/// form back (`"0.3"`, `"5"`), which is also how the type serialises
/// inside [`crate::OrderQuoteResponse`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProtocolFeeBps(u64);

impl ProtocolFeeBps {
    /// Zero protocol fee: skips the protocol-fee leg entirely.
    pub const ZERO: Self = Self(0);

    /// The internal `bps * 100_000` representation (`"0.3"` is
    /// `30_000`, `"5"` is `500_000`). Pair with
    /// [`protocol_fee_bps_scale`] when reasoning about precision.
    pub const fn scaled(self) -> u64 {
        self.0
    }
}

impl FromStr for ProtocolFeeBps {
    type Err = Error;

    /// Parse the orderbook's decimal `protocolFeeBps` string. Empty or
    /// whitespace-only input is treated as zero, matching how the
    /// orderbook omits the field for fee-free quotes. The sixth
    /// fractional digit rounds half-away-from-zero, matching the TS
    /// SDK's `Math.round(protocolFeeBps * 100_000)`.
    fn from_str(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(Self::ZERO);
        }
        if trimmed.starts_with('-') {
            return Err(Error::InvalidProtocolFeeBps {
                value: raw.to_owned(),
                reason: "must be non-negative",
            });
        }
        let (int_part, frac_part) = trimmed
            .find('.')
            .map_or((trimmed, ""), |i| (&trimmed[..i], &trimmed[i + 1..]));
        if int_part.is_empty() && frac_part.is_empty() {
            return Err(Error::InvalidProtocolFeeBps {
                value: raw.to_owned(),
                reason: "empty value",
            });
        }
        let int_val: u64 = if int_part.is_empty() {
            0
        } else {
            int_part.parse().map_err(|_| Error::InvalidProtocolFeeBps {
                value: raw.to_owned(),
                reason: "integer part is not a decimal number",
            })?
        };
        let frac_scaled = if frac_part.is_empty() {
            0u64
        } else {
            if !frac_part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Error::InvalidProtocolFeeBps {
                    value: raw.to_owned(),
                    reason: "fractional part is not a decimal number",
                });
            }
            round_fraction_to_five_digits(frac_part.as_bytes())
        };
        int_val
            .checked_mul(HUNDRED_THOUSANDS)
            .and_then(|v| v.checked_add(frac_scaled))
            .map(Self)
            .ok_or(Error::InvalidProtocolFeeBps {
                value: raw.to_owned(),
                reason: "value too large for internal scale",
            })
    }
}

impl fmt::Display for ProtocolFeeBps {
    /// Render the wire decimal back: `30_000` formats as `"0.3"`,
    /// `500_000` as `"5"`. Trailing fractional zeros are trimmed, so
    /// the output is canonical rather than byte-identical to whatever
    /// padding the orderbook sent.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let int = self.0 / HUNDRED_THOUSANDS;
        let frac = self.0 % HUNDRED_THOUSANDS;
        if frac == 0 {
            return write!(f, "{int}");
        }
        let padded = format!("{frac:05}");
        write!(f, "{int}.{}", padded.trim_end_matches('0'))
    }
}

/// Partner-fee, slippage, and protocol-fee inputs for the quote
/// projection: the single public costs type consumed by
/// [`crate::OrderQuoteResponse::try_to_order_data`] and the quote
/// pipeline.
///
/// [`Default`] is neutral (no partner fee, no slippage, no override),
/// so `&OrderCosts::default()` reproduces the raw [api §"Step 3"]
/// adjustment. The pipeline's quote builder seeds
/// [`DEFAULT_SLIPPAGE_BPS`] on top; that recommendation deliberately
/// lives in the named constant, not here.
///
/// [api §"Step 3"]: https://docs.cow.fi/cow-protocol/howto/integrate/api#step-3-compute-the-amounts-to-sign
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OrderCosts {
    /// Partner-fee tier in basis points, charged on the surplus side.
    /// `0` skips the partner-fee leg.
    pub partner_fee_bps: u32,
    /// Slippage tolerance in basis points, applied to the non-fixed
    /// side of the signed order (`buy_amount` for SELL, `sell_amount`
    /// for BUY). `0` signs the raw quote.
    pub slippage_bps: u32,
    /// Override for the `protocolFeeBps` echoed by the quote response.
    /// `None` falls back to [`crate::OrderQuoteResponse::protocol_fee_bps`].
    pub protocol_fee_bps_override: Option<ProtocolFeeBps>,
}

/// A pair of sell / buy amounts at a specific fee-application stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Amounts {
    /// `sellAmount` at this stage, in atomic units.
    pub sell_amount: U256,
    /// `buyAmount` at this stage, in atomic units.
    pub buy_amount: U256,
}

/// Cost breakdown for a single quote, in the same shape the TS SDK's
/// `QuoteAmountsAndCosts.costs` exposes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuoteCosts {
    /// Network cost in `sell_token` atomic units (echoes the quote's
    /// `feeAmount`).
    pub network_fee_in_sell: U256,
    /// Network cost projected into `buy_token` atomic units via the
    /// quote's price ratio.
    pub network_fee_in_buy: U256,
    /// Absolute partner-fee amount, in the surplus token
    /// (`buy_token` for SELL orders, `sell_token` for BUY orders).
    pub partner_fee_amount: U256,
    /// Partner fee in basis points (0 if no partner fee was applied).
    pub partner_fee_bps: u32,
    /// Absolute protocol-fee amount, in the surplus token.
    pub protocol_fee_amount: U256,
    /// Protocol fee scaled by `100_000` (i.e. `5` for an integer
    /// bps value of `5`, `30_000` for `"0.3"`). Use
    /// [`protocol_fee_bps_scale`] when reasoning about precision.
    pub protocol_fee_bps_scaled: u64,
}

/// Full amount projection plus cost breakdown, mirroring the TS SDK's
/// `QuoteAmountsAndCosts` return type.
///
/// The intermediate stages (`before_all_fees`, `after_protocol_fees`,
/// `after_network_costs`, `after_partner_fees`, `after_slippage`) and
/// `costs` reproduce the cow-sdk TypeScript `QuoteAmountsAndCosts` surface
/// field-for-field, for parity with that reference and so callers can
/// inspect any leg of the fee composition. Only `amounts_to_sign` is
/// consumed internally (by
/// [`crate::OrderQuoteResponse::try_to_order_data`]); the other
/// stages are exposed for parity and external inspection, not because the
/// SDK reads them back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuoteAmountsAndCosts {
    /// `true` for SELL quotes, `false` for BUY.
    pub is_sell: bool,
    /// Amounts at the spot-price stage: protocol fee added back on
    /// SELL, removed on BUY. Network costs are *included* on the sell
    /// side for SELL orders and *not* added on the buy side for BUY
    /// orders, exactly as the orderbook reports them.
    pub before_all_fees: Amounts,
    /// Amounts after the protocol fee but before network costs.
    pub after_protocol_fees: Amounts,
    /// Amounts as returned by the `/quote` endpoint (for SELL) or with
    /// network costs added to `sellAmount` (for BUY).
    pub after_network_costs: Amounts,
    /// Amounts after deducting partner fee from the surplus side.
    pub after_partner_fees: Amounts,
    /// Amounts after slippage is applied to the non-fixed side.
    pub after_slippage: Amounts,
    /// The amounts that should be signed and submitted: SELL uses
    /// `before_all_fees.sell` and `after_slippage.buy`; BUY uses
    /// `after_slippage.sell` and `before_all_fees.buy`.
    pub amounts_to_sign: Amounts,
    /// Cost breakdown.
    pub costs: QuoteCosts,
}

/// Inputs to [`compute`].
#[derive(Clone, Copy, Debug)]
pub struct QuoteAmountsParams {
    /// Quote direction (SELL or BUY).
    pub kind: OrderKind,
    /// `quote.sellAmount` from the orderbook response.
    pub sell_amount: U256,
    /// `quote.buyAmount` from the orderbook response.
    pub buy_amount: U256,
    /// `quote.feeAmount` (network cost) from the response.
    pub fee_amount: U256,
    /// Partner fee in basis points, applied on top of the protocol
    /// adjustment. `0` skips the partner-fee leg.
    pub partner_fee_bps: u32,
    /// Slippage tolerance in basis points, applied to the non-fixed
    /// side (SELL: buy; BUY: sell). `0` skips the slippage leg.
    pub slippage_bps: u32,
    /// `protocolFeeBps` echoed by the quote. `None` and
    /// `Some(ProtocolFeeBps::ZERO)` are equivalent and skip the
    /// protocol-fee leg.
    pub protocol_fee_bps: Option<ProtocolFeeBps>,
}

/// Compute every fee-application stage and the `amountsToSign` for a
/// quote response, matching the TS SDK's `getQuoteAmountsAndCosts`.
///
/// Returns [`Error::QuoteSellAmountZero`] when the quote reports
/// `sellAmount = 0`.
///
/// ```
/// use alloy_primitives::U256;
/// use cowprotocol_orderbook::{OrderKind, quote_amounts};
///
/// let res = quote_amounts::compute(quote_amounts::QuoteAmountsParams {
///     kind: OrderKind::Sell,
///     sell_amount: U256::from(1_000_000_000_000_000_000u128),
///     buy_amount: U256::from(2_000_000_000_000_000_000u128),
///     fee_amount: U256::ZERO,
///     partner_fee_bps: 100,
///     slippage_bps: 50,
///     protocol_fee_bps: Some("5".parse()?),
/// })?;
/// assert_eq!(
///     res.amounts_to_sign.buy_amount,
///     U256::from(1_970_090_045_022_511_257u128),
/// );
/// # Ok::<(), cowprotocol_orderbook::Error>(())
/// ```
pub fn compute(params: QuoteAmountsParams) -> Result<QuoteAmountsAndCosts> {
    let QuoteAmountsParams {
        kind,
        sell_amount,
        buy_amount,
        fee_amount,
        partner_fee_bps,
        slippage_bps,
        protocol_fee_bps,
    } = params;

    let is_sell = matches!(kind, OrderKind::Sell);
    let protocol_fee_bps_scaled = protocol_fee_bps.unwrap_or_default().scaled();

    if sell_amount.is_zero() {
        return Err(Error::QuoteSellAmountZero);
    }
    let network_fee_in_buy = mul_div(
        buy_amount,
        fee_amount,
        sell_amount,
        "network_fee_in_buy.mul_div",
    )?;

    let protocol_fee_amount = protocol_fee_amount(
        kind,
        sell_amount,
        buy_amount,
        fee_amount,
        protocol_fee_bps_scaled,
    )?;

    // The two directions are mirror images: fees always land on the
    // surplus side while the fixed side is pinned. Work in (fixed,
    // surplus) pairs (SELL: fixed=sell, surplus=buy; BUY: fixed=buy,
    // surplus=sell), so the protocol/partner/slippage/sign legs collapse
    // to a single body. `before_all_fees` and `after_network_costs` stay
    // direction-specific because the network cost lands asymmetrically:
    // baked into the SELL sell side, added to the BUY sell side later.
    let dir = Dir::new(is_sell);

    let before_all_fees = if is_sell {
        // Spot price: undo the protocol fee and network cost the API
        // folded into the quoted surplus side, plus the network cost on
        // the fixed side.
        let fixed = checked_add(sell_amount, fee_amount, "before_all_fees.sell")?;
        let surplus = checked_add(
            buy_amount,
            network_fee_in_buy,
            "before_all_fees.buy_network",
        )?;
        let surplus = checked_add(surplus, protocol_fee_amount, "before_all_fees.buy_protocol")?;
        dir.amounts(fixed, surplus)
    } else {
        let surplus = checked_sub(
            sell_amount,
            protocol_fee_amount,
            "before_all_fees.sell_protocol",
        )?;
        dir.amounts(buy_amount, surplus)
    };

    // Protocol fee leaves the surplus side: removed on SELL, added back on
    // BUY. The fixed side is pinned to its `before_all_fees` value.
    let after_protocol_fees = dir.amounts(
        dir.fixed_of(&before_all_fees),
        dir.peel_fee(
            dir.surplus_of(&before_all_fees),
            protocol_fee_amount,
            dir.after_protocol_stage,
        )?,
    );

    // Network costs are already baked into the SELL quote, so this leg is a
    // pure identity there; on BUY it adds the network fee to the sell side.
    let after_network_costs = if is_sell {
        dir.amounts(sell_amount, buy_amount)
    } else {
        let fixed = dir.fixed_of(&after_protocol_fees);
        let surplus = checked_add(
            dir.surplus_of(&after_protocol_fees),
            fee_amount,
            "after_network_costs.sell",
        )?;
        dir.amounts(fixed, surplus)
    };

    // Partner fee is charged on the spot-price surplus base.
    let partner_fee_amount = if partner_fee_bps == 0 {
        U256::ZERO
    } else {
        mul_div(
            dir.surplus_of(&before_all_fees),
            U256::from(partner_fee_bps),
            U256::from(ONE_HUNDRED_BPS),
            "partner_fee.mul_div",
        )?
    };
    let after_partner_fees = dir.amounts(
        dir.fixed_of(&after_network_costs),
        dir.peel_fee(
            dir.surplus_of(&after_network_costs),
            partner_fee_amount,
            dir.after_partner_stage,
        )?,
    );

    // Slippage widens the surplus side against the trader: tightening the
    // SELL buy floor, loosening the BUY sell ceiling.
    let surplus_after_partner = dir.surplus_of(&after_partner_fees);
    let slip = mul_div(
        surplus_after_partner,
        U256::from(slippage_bps),
        U256::from(ONE_HUNDRED_BPS),
        dir.slippage_mul_div_stage,
    )?;
    let after_slippage = dir.amounts(
        dir.fixed_of(&after_partner_fees),
        dir.peel_fee(surplus_after_partner, slip, dir.after_slippage_stage)?,
    );

    // Sign the pinned fixed side and the fully-adjusted surplus side.
    let amounts_to_sign = dir.amounts(
        dir.fixed_of(&before_all_fees),
        dir.surplus_of(&after_slippage),
    );

    Ok(QuoteAmountsAndCosts {
        is_sell,
        before_all_fees,
        after_protocol_fees,
        after_network_costs,
        after_partner_fees,
        after_slippage,
        amounts_to_sign,
        costs: QuoteCosts {
            network_fee_in_sell: fee_amount,
            network_fee_in_buy,
            partner_fee_amount,
            partner_fee_bps,
            protocol_fee_amount,
            protocol_fee_bps_scaled,
        },
    })
}

/// Maps the direction-agnostic `(fixed, surplus)` projection back onto the
/// `(sell, buy)` wire pair and carries the per-leg overflow-stage labels.
///
/// On a SELL quote the sell side is fixed and the buy side carries the
/// surplus (and every fee); on a BUY quote the roles swap. `peel_fee`
/// subtracts a fee from the surplus on SELL and adds it on BUY, which is
/// the single sign difference between the two directions once the network
/// cost (handled separately in [`compute`]) is set aside.
#[derive(Clone, Copy)]
struct Dir {
    is_sell: bool,
    after_protocol_stage: &'static str,
    after_partner_stage: &'static str,
    slippage_mul_div_stage: &'static str,
    after_slippage_stage: &'static str,
}

impl Dir {
    const fn new(is_sell: bool) -> Self {
        if is_sell {
            Self {
                is_sell,
                after_protocol_stage: "after_protocol_fees.buy",
                after_partner_stage: "after_partner_fees.buy",
                slippage_mul_div_stage: "slippage_buy.mul_div",
                after_slippage_stage: "after_slippage.buy",
            }
        } else {
            Self {
                is_sell,
                after_protocol_stage: "after_protocol_fees.sell",
                after_partner_stage: "after_partner_fees.sell",
                slippage_mul_div_stage: "slippage_sell.mul_div",
                after_slippage_stage: "after_slippage.sell",
            }
        }
    }

    /// Assemble an [`Amounts`] from a `(fixed, surplus)` pair.
    const fn amounts(&self, fixed: U256, surplus: U256) -> Amounts {
        if self.is_sell {
            Amounts {
                sell_amount: fixed,
                buy_amount: surplus,
            }
        } else {
            Amounts {
                sell_amount: surplus,
                buy_amount: fixed,
            }
        }
    }

    /// The pinned (non-surplus) side of `a`.
    const fn fixed_of(&self, a: &Amounts) -> U256 {
        if self.is_sell {
            a.sell_amount
        } else {
            a.buy_amount
        }
    }

    /// The fee-bearing surplus side of `a`.
    const fn surplus_of(&self, a: &Amounts) -> U256 {
        if self.is_sell {
            a.buy_amount
        } else {
            a.sell_amount
        }
    }

    /// Move `fee` off the surplus side: it leaves the trader's proceeds on
    /// SELL (subtract) and is added to what they pay on BUY (add).
    fn peel_fee(&self, surplus: U256, fee: U256, stage: &'static str) -> Result<U256> {
        if self.is_sell {
            checked_sub(surplus, fee, stage)
        } else {
            checked_add(surplus, fee, stage)
        }
    }
}

/// Checked `U256` addition that fails closed at `stage` on overflow.
#[inline]
fn checked_add(a: U256, b: U256, stage: &'static str) -> Result<U256> {
    a.checked_add(b)
        .ok_or(Error::QuoteFeeMathOverflow { stage })
}

/// Checked `U256` subtraction that fails closed at `stage` on underflow.
#[inline]
fn checked_sub(a: U256, b: U256, stage: &'static str) -> Result<U256> {
    a.checked_sub(b)
        .ok_or(Error::QuoteFeeMathOverflow { stage })
}

/// Scale factor applied to `protocol_fee_bps` to keep five decimal
/// digits of precision (10⁵). Exported so consumers that compute their
/// own protocol-fee amounts use the same denominator the SDK does.
pub const fn protocol_fee_bps_scale() -> u64 {
    HUNDRED_THOUSANDS
}

/// Read `frac_digits` (ASCII decimal digits, no sign or point) as a
/// fixed-point value scaled to five fractional places, rounding the sixth
/// place half-away-from-zero to match the TS SDK's
/// `Math.round(protocolFeeBps * 100_000)`.
///
/// The first five digits form the scaled integer; if a sixth digit exists
/// and is `>= 5`, the result is incremented. Digits beyond the sixth do not
/// affect the outcome (a `Math.round` of a value already past the rounding
/// boundary). Callers must validate that every byte is an ASCII digit.
fn round_fraction_to_five_digits(frac_digits: &[u8]) -> u64 {
    let digit_at = |i: usize| u64::from(frac_digits.get(i).map_or(0, |b| b - b'0'));
    let five = (0..5).fold(0u64, |acc, i| acc * 10 + digit_at(i));
    if digit_at(5) >= 5 { five + 1 } else { five }
}

fn protocol_fee_amount(
    kind: OrderKind,
    sell_amount: U256,
    buy_amount: U256,
    fee_amount: U256,
    bps_scaled: u64,
) -> Result<U256> {
    if bps_scaled == 0 {
        return Ok(U256::ZERO);
    }
    let bps_big = U256::from(bps_scaled);
    match kind {
        OrderKind::Sell => {
            // protocolFeeInBuy = buyAmount * bps / (10_000 * 100_000 - bps)
            // bps >= SCALE means the quote claims >=100% protocol fee,
            // which the TS reference would surface as division by zero
            // (or worse, negative denominator). Fail closed instead of
            // letting an over-100% fee saturate to a bogus signed amount.
            let denom = SCALE
                .checked_sub(bps_big)
                .ok_or(Error::QuoteFeeMathOverflow {
                    stage: "protocol_fee.sell_denom",
                })?;
            if denom.is_zero() {
                return Err(Error::QuoteFeeMathOverflow {
                    stage: "protocol_fee.sell_denom",
                });
            }
            mul_div(buy_amount, bps_big, denom, "protocol_fee.sell_mul_div")
        }
        OrderKind::Buy => {
            // protocolFeeInSell = (sellAmount + feeAmount) * bps
            //                     / (10_000 * 100_000 + bps)
            // `SCALE + bps` cannot zero (SCALE > 0), so no zero check.
            let denom = SCALE
                .checked_add(bps_big)
                .ok_or(Error::QuoteFeeMathOverflow {
                    stage: "protocol_fee.buy_denom",
                })?;
            let base = sell_amount
                .checked_add(fee_amount)
                .ok_or(Error::QuoteFeeMathOverflow {
                    stage: "protocol_fee.buy_base",
                })?;
            mul_div(base, bps_big, denom, "protocol_fee.buy_mul_div")
        }
    }
}

/// `a * b / c`, failing closed on multiplication overflow.
///
/// `c` is an internal invariant: every caller proves it non-zero before
/// calling (the bps denominators are non-zero consts, `protocol_fee_amount`
/// rejects a zero `denom` at its own boundary, and `network_fee_in_buy`
/// passes a guarded-non-zero `sell_amount`). A zero `c` therefore signals a
/// caller bug, not malformed input, so it trips a `debug_assert!` rather
/// than masking a future divide-by-zero behind a silent `0`.
#[inline]
fn mul_div(a: U256, b: U256, c: U256, stage: &'static str) -> Result<U256> {
    debug_assert!(
        !c.is_zero(),
        "mul_div denominator must be non-zero at {stage}"
    );
    let prod = a
        .checked_mul(b)
        .ok_or(Error::QuoteFeeMathOverflow { stage })?;
    Ok(prod / c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks the exact divergence the upstream TypeScript SDK fix
    /// (`cow-sdk` #867, "fix(trading): take protocol fee into account")
    /// pinned in `packages/trading/src/postCoWProtocolTrade.test.ts`.
    ///
    /// Default quote: `sellAmount = 1e18`, `buyAmount = 2e18`,
    /// `feeAmount = 0`, kind = SELL, slippage = 50 bps, partner fee =
    /// 100 bps. With `protocolFeeBps = 0` the signed `buyAmount` is
    /// `1_970_100_000_000_000_000`; with `protocolFeeBps = 5` it
    /// becomes `1_970_090_045_022_511_257`. Both numbers are checked
    /// here against our port.
    #[test]
    fn matches_ts_pr_867_partner_plus_protocol_fee_divergence() {
        let base = QuoteAmountsParams {
            kind: OrderKind::Sell,
            sell_amount: U256::from(1_000_000_000_000_000_000u128),
            buy_amount: U256::from(2_000_000_000_000_000_000u128),
            fee_amount: U256::ZERO,
            partner_fee_bps: 100,
            slippage_bps: 50,
            protocol_fee_bps: None,
        };

        let without = compute(base).unwrap();
        assert_eq!(
            without.amounts_to_sign.buy_amount,
            U256::from(1_970_100_000_000_000_000u128),
            "buyAmount without protocol fee must match TS PR #867 baseline",
        );

        let with_protocol = compute(QuoteAmountsParams {
            protocol_fee_bps: Some("5".parse().unwrap()),
            ..base
        })
        .unwrap();
        assert_eq!(
            with_protocol.amounts_to_sign.buy_amount,
            U256::from(1_970_090_045_022_511_257u128),
            "buyAmount with protocolFeeBps=5 must match TS PR #867 expected value",
        );
        assert!(
            with_protocol.amounts_to_sign.buy_amount < without.amounts_to_sign.buy_amount,
            "protocol fee enlarges the partner-fee base, so signed buy_amount must drop",
        );
    }

    #[test]
    fn sell_order_no_fees_passes_amounts_through_to_signing() {
        let res = compute(QuoteAmountsParams {
            kind: OrderKind::Sell,
            sell_amount: U256::from(1_000_000_000_000_000_000u128),
            buy_amount: U256::from(2_000_000_000_000_000_000u128),
            fee_amount: U256::ZERO,
            partner_fee_bps: 0,
            slippage_bps: 0,
            protocol_fee_bps: None,
        })
        .unwrap();
        assert_eq!(
            res.amounts_to_sign.sell_amount,
            U256::from(1_000_000_000_000_000_000u128)
        );
        assert_eq!(
            res.amounts_to_sign.buy_amount,
            U256::from(2_000_000_000_000_000_000u128)
        );
        assert_eq!(res.costs.partner_fee_amount, U256::ZERO);
        assert_eq!(res.costs.protocol_fee_amount, U256::ZERO);
    }

    #[test]
    fn sell_order_folds_fee_amount_into_signed_sell() {
        let res = compute(QuoteAmountsParams {
            kind: OrderKind::Sell,
            sell_amount: U256::from(1_000_000_000_000_000_000u128),
            buy_amount: U256::from(2_000_000_000_000_000_000u128),
            fee_amount: U256::from(1_000_000_000_000_000u128),
            partner_fee_bps: 0,
            slippage_bps: 0,
            protocol_fee_bps: None,
        })
        .unwrap();
        assert_eq!(
            res.amounts_to_sign.sell_amount,
            U256::from(1_001_000_000_000_000_000u128),
        );
    }

    #[test]
    fn buy_order_applies_slippage_to_sell_side() {
        let res = compute(QuoteAmountsParams {
            kind: OrderKind::Buy,
            sell_amount: U256::from(1_000_000_000_000_000_000u128),
            buy_amount: U256::from(2_000_000_000_000_000_000u128),
            fee_amount: U256::ZERO,
            partner_fee_bps: 0,
            slippage_bps: 50,
            protocol_fee_bps: None,
        })
        .unwrap();
        // sell becomes sell * (1 + 0.005) = 1.005e18; buy passes through.
        assert_eq!(
            res.amounts_to_sign.sell_amount,
            U256::from(1_005_000_000_000_000_000u128),
        );
        assert_eq!(
            res.amounts_to_sign.buy_amount,
            U256::from(2_000_000_000_000_000_000u128)
        );
    }

    fn parse(raw: &str) -> Result<u64> {
        raw.parse::<ProtocolFeeBps>().map(ProtocolFeeBps::scaled)
    }

    #[test]
    fn protocol_fee_bps_accepts_decimal_string() {
        assert_eq!(parse("").unwrap(), 0);
        assert_eq!(parse("0").unwrap(), 0);
        assert_eq!(parse("5").unwrap(), 500_000);
        assert_eq!(parse("0.3").unwrap(), 30_000);
        assert_eq!(parse("0.30000").unwrap(), 30_000);
        // Round-half-away-from-zero, matching `Math.round`.
        assert_eq!(parse("0.000005").unwrap(), 1);
        assert_eq!(parse("0.000004").unwrap(), 0);
    }

    #[test]
    fn protocol_fee_bps_rejects_negative_or_garbage() {
        for garbage in ["-1", "abc", "0.x"] {
            assert!(matches!(
                parse(garbage),
                Err(Error::InvalidProtocolFeeBps { .. }),
            ));
        }
    }

    #[test]
    fn protocol_fee_bps_display_renders_wire_decimal() {
        for (raw, rendered) in [("5", "5"), ("0.3", "0.3"), ("0.30000", "0.3"), ("0", "0")] {
            assert_eq!(raw.parse::<ProtocolFeeBps>().unwrap().to_string(), rendered);
        }
        assert_eq!(ProtocolFeeBps::ZERO.to_string(), "0");
    }

    /// `sell + fee` overflow on a SELL quote must surface as
    /// [`Error::QuoteFeeMathOverflow`] at the `before_all_fees.sell`
    /// stage rather than saturating to `U256::MAX` and being copied
    /// into the signed `OrderData`. The same contract covers
    /// [`crate::OrderQuoteResponse::try_to_order_data`], which routes
    /// through [`compute`].
    #[test]
    fn rejects_sell_plus_fee_overflow() {
        let err = compute(QuoteAmountsParams {
            kind: OrderKind::Sell,
            sell_amount: U256::MAX,
            buy_amount: U256::from(1u64),
            fee_amount: U256::from(1u64),
            partner_fee_bps: 0,
            slippage_bps: 0,
            protocol_fee_bps: None,
        })
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::QuoteFeeMathOverflow {
                    stage: "before_all_fees.sell"
                },
            ),
            "got: {err:?}",
        );
    }

    /// A SELL quote that reports `buyAmount = U256::MAX` together with
    /// a non-zero `protocolFeeBps` would saturate the protocol-fee
    /// `mul_div` to a bogus value before signing. The fail-closed
    /// contract requires erroring out instead.
    #[test]
    fn rejects_protocol_fee_mul_div_overflow_on_sell() {
        let err = compute(QuoteAmountsParams {
            kind: OrderKind::Sell,
            sell_amount: U256::from(1u64),
            buy_amount: U256::MAX,
            fee_amount: U256::ZERO,
            partner_fee_bps: 0,
            slippage_bps: 0,
            protocol_fee_bps: Some("5".parse().unwrap()),
        })
        .unwrap_err();
        assert!(
            matches!(err, Error::QuoteFeeMathOverflow { .. }),
            "got: {err:?}",
        );
    }

    /// Slippage above 100% would clamp `after_slippage.buy_amount` to
    /// zero under saturating arithmetic, producing an "any price
    /// accepted" SELL order. The checked path must reject it.
    #[test]
    fn rejects_slippage_above_one_hundred_percent_on_sell() {
        let err = compute(QuoteAmountsParams {
            kind: OrderKind::Sell,
            sell_amount: U256::from(1_000_000_000_000_000_000u128),
            buy_amount: U256::from(1_000_000_000_000_000_000u128),
            fee_amount: U256::ZERO,
            partner_fee_bps: 0,
            slippage_bps: 20_000,
            protocol_fee_bps: None,
        })
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::QuoteFeeMathOverflow {
                    stage: "after_slippage.buy"
                },
            ),
            "got: {err:?}",
        );
    }

    /// On a BUY order, an inflated `protocolFeeBps` plus a `feeAmount`
    /// larger than the `sellAmount` makes `protocol_fee_amount` exceed
    /// `sell_amount`, which would underflow to zero under saturating
    /// arithmetic. Checked subtraction surfaces the failure.
    #[test]
    fn rejects_buy_protocol_fee_underflowing_sell_amount() {
        let err = compute(QuoteAmountsParams {
            kind: OrderKind::Buy,
            sell_amount: U256::from(10u64),
            buy_amount: U256::from(1u64),
            fee_amount: U256::from(1_000u64),
            partner_fee_bps: 0,
            slippage_bps: 0,
            protocol_fee_bps: Some("9999.99999".parse().unwrap()),
        })
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::QuoteFeeMathOverflow {
                    stage: "before_all_fees.sell_protocol"
                },
            ),
            "got: {err:?}",
        );
    }

    /// `protocolFeeBps >= 100%` makes the SELL-side denominator
    /// `scale - bps` zero or negative. The TS reference would divide
    /// by zero; we reject it explicitly.
    #[test]
    fn rejects_protocol_fee_bps_at_or_above_one_hundred_percent() {
        let err_at = compute(QuoteAmountsParams {
            kind: OrderKind::Sell,
            sell_amount: U256::from(1_000_000_000_000_000_000u128),
            buy_amount: U256::from(1_000_000_000_000_000_000u128),
            fee_amount: U256::ZERO,
            partner_fee_bps: 0,
            slippage_bps: 0,
            // 10_000 bps == scale, denom collapses to zero.
            protocol_fee_bps: Some("10000".parse().unwrap()),
        })
        .unwrap_err();
        assert!(
            matches!(
                err_at,
                Error::QuoteFeeMathOverflow {
                    stage: "protocol_fee.sell_denom"
                },
            ),
            "got: {err_at:?}",
        );

        let err_above = compute(QuoteAmountsParams {
            kind: OrderKind::Sell,
            sell_amount: U256::from(1_000_000_000_000_000_000u128),
            buy_amount: U256::from(1_000_000_000_000_000_000u128),
            fee_amount: U256::ZERO,
            partner_fee_bps: 0,
            slippage_bps: 0,
            // 10_001 bps > scale, denom underflows.
            protocol_fee_bps: Some("10001".parse().unwrap()),
        })
        .unwrap_err();
        assert!(
            matches!(
                err_above,
                Error::QuoteFeeMathOverflow {
                    stage: "protocol_fee.sell_denom"
                },
            ),
            "got: {err_above:?}",
        );
    }

    #[test]
    fn zero_sell_amount_is_refused_instead_of_dividing_by_zero() {
        let err = compute(QuoteAmountsParams {
            kind: OrderKind::Sell,
            sell_amount: U256::ZERO,
            buy_amount: U256::from(1_u64),
            fee_amount: U256::ZERO,
            partner_fee_bps: 0,
            slippage_bps: 0,
            protocol_fee_bps: None,
        })
        .unwrap_err();
        assert!(matches!(err, Error::QuoteSellAmountZero));
    }
}
