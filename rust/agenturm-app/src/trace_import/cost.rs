//! Selective projection of explicit producer-reported amounts.

use super::{Attribute, AttributeValue};
use agenturm_kernel::NANOUNITS_PER_CURRENCY_UNIT;

pub(crate) const AMOUNT: &str = "agenturm.reported_cost.amount";
pub(crate) const CURRENCY: &str = "agenturm.reported_cost.currency";
pub(crate) const GEN_AI_AMOUNT: &str = "gen_ai.usage.cost";
pub(crate) const GEN_AI_CURRENCY: &str = "gen_ai.usage.cost.currency";
pub(crate) const LLM_AMOUNT: &str = "llm.cost.total";
pub(crate) const LLM_CURRENCY: &str = "llm.cost.currency";
pub(crate) const USD_AMOUNT: &str = "llm.cost_usd";
pub(crate) const KNOWN: &str = "llm.cost_known";

pub(crate) fn is_cost_key(key: &str) -> bool {
    matches!(
        key,
        AMOUNT
            | CURRENCY
            | GEN_AI_AMOUNT
            | GEN_AI_CURRENCY
            | LLM_AMOUNT
            | LLM_CURRENCY
            | USD_AMOUNT
            | KNOWN
    )
}

pub(super) fn reported_amount(attributes: &[Attribute]) -> Option<(String, u64)> {
    let has = |key| attributes.iter().any(|attribute| attribute.key == key);
    for (amount, currency) in [
        (AMOUNT, CURRENCY),
        (GEN_AI_AMOUNT, GEN_AI_CURRENCY),
        (LLM_AMOUNT, LLM_CURRENCY),
    ] {
        if has(amount) && !has(currency) {
            return None;
        }
    }
    let mut amount = None;
    let mut currency = None;
    for attribute in attributes.iter().filter(|attr| is_cost_key(&attr.key)) {
        let value = attribute.value.as_ref()?;
        match attribute.key.as_str() {
            KNOWN
                if value.bool_value.as_bool() == Some(true)
                    && value.string_value.is_null()
                    && value.int_value.is_none()
                    && value.double_value.is_none() => {}
            KNOWN => return None,
            CURRENCY | GEN_AI_CURRENCY | LLM_CURRENCY => {
                let unit = value.string_value.as_str()?;
                if !value.bool_value.is_null()
                    || value.int_value.is_some()
                    || value.double_value.is_some()
                    || unit.len() != 3
                    || !unit.bytes().all(|byte| byte.is_ascii_uppercase())
                {
                    return None;
                }
                agree(&mut currency, unit)?;
            }
            key => {
                if key == USD_AMOUNT {
                    agree(&mut currency, "USD")?;
                }
                agree(&mut amount, nanounits(value)?)?;
            }
        }
    }
    Some((currency?.to_owned(), amount?))
}

fn agree<T: PartialEq>(slot: &mut Option<T>, value: T) -> Option<()> {
    if slot.as_ref().is_some_and(|prior| prior != &value) {
        return None;
    }
    *slot = Some(value);
    Some(())
}

fn nanounits(value: &AttributeValue) -> Option<u64> {
    if !value.bool_value.is_null() {
        return None;
    }
    match (&value.string_value, &value.int_value, &value.double_value) {
        (serde_json::Value::String(decimal), None, None) => parse_decimal(decimal),
        (serde_json::Value::Null, Some(integer), None) => {
            let integer = integer.get();
            let literal = if integer.starts_with('"') {
                serde_json::from_str::<String>(integer)
                    .ok()?
                    .parse::<u64>()
                    .ok()?
            } else {
                integer.parse::<u64>().ok()?
            };
            literal
                .checked_mul(NANOUNITS_PER_CURRENCY_UNIT)
                .filter(|value| *value <= i64::MAX as u64)
        }
        (serde_json::Value::Null, None, Some(decimal)) => parse_decimal(decimal.get()),
        _ => None,
    }
}

fn parse_decimal(value: &str) -> Option<u64> {
    if value.is_empty() || value.len() > 48 {
        return None;
    }
    let (mantissa, exponent) = match value.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (value, 0),
    };
    if !(-64..=64).contains(&exponent) {
        return None;
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{whole}{fraction}");
    let significant = digits.trim_end_matches('0').trim_start_matches('0');
    if significant.is_empty() {
        return Some(0);
    }
    let trailing = digits.len() - digits.trim_end_matches('0').len();
    let integer_scale = (NANOUNITS_PER_CURRENCY_UNIT.ilog10() as i32)
        .checked_add(exponent)?
        .checked_add(trailing as i32)?
        .checked_sub(fraction.len() as i32)?;
    let scale = 10u64.checked_pow(u32::try_from(integer_scale).ok()?)?;
    let result = significant.parse::<u64>().ok()?.checked_mul(scale)?;
    (result <= i64::MAX as u64).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_preserves_precision_and_explicit_zero() {
        for (value, expected) in [
            ("0", Some(0)),
            ("0.0000000000", Some(0)),
            ("2.501", Some(2_501_000_000)),
            ("1.23E-7", Some(123)),
            ("0.0000000001", None),
            ("1e-400", None),
            ("9223372036.854775807", Some(i64::MAX as u64)),
            ("9223372036.854775808", None),
            ("1e1000000", None),
            ("-0.001", None),
            ("Infinity", None),
        ] {
            assert_eq!(parse_decimal(value), expected);
        }
    }
}
