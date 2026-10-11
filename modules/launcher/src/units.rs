//! Unit conversion: `10 km to mi`, `5 kg in lb`, `100 f to c`, `2 gb to mb`.

use crate::model::{Provider, ResultKind, SearchResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Length,
    Mass,
    Volume,
    Time,
    Data,
    Speed,
    Area,
    Temperature,
}

struct Unit {
    /// Lower-case spellings the user may type.
    names: &'static [&'static str],
    /// Short label used in the answer.
    symbol: &'static str,
    category: Category,
    /// Multiply by this to reach the category's base unit (ignored for temperature).
    factor: f64,
}

const fn unit(
    names: &'static [&'static str],
    symbol: &'static str,
    category: Category,
    factor: f64,
) -> Unit {
    Unit { names, symbol, category, factor }
}

use Category::*;

static UNITS: &[Unit] = &[
    // Length (base: metre)
    unit(&["mm", "millimeter", "millimeters", "millimetre", "millimetres"], "mm", Length, 0.001),
    unit(&["cm", "centimeter", "centimeters", "centimetre", "centimetres"], "cm", Length, 0.01),
    unit(&["m", "meter", "meters", "metre", "metres"], "m", Length, 1.0),
    unit(&["km", "kilometer", "kilometers", "kilometre", "kilometres"], "km", Length, 1000.0),
    unit(&["in", "inch", "inches", "\""], "in", Length, 0.0254),
    unit(&["ft", "foot", "feet", "'"], "ft", Length, 0.3048),
    unit(&["yd", "yard", "yards"], "yd", Length, 0.9144),
    unit(&["mi", "mile", "miles"], "mi", Length, 1609.344),
    unit(&["nmi", "nauticalmile", "nauticalmiles"], "nmi", Length, 1852.0),
    // Mass (base: kilogram)
    unit(&["mg", "milligram", "milligrams"], "mg", Mass, 1e-6),
    unit(&["g", "gram", "grams"], "g", Mass, 0.001),
    unit(&["kg", "kilogram", "kilograms", "kilo", "kilos"], "kg", Mass, 1.0),
    unit(&["t", "tonne", "tonnes"], "t", Mass, 1000.0),
    unit(&["oz", "ounce", "ounces"], "oz", Mass, 0.028349523125),
    unit(&["lb", "lbs", "pound", "pounds"], "lb", Mass, 0.45359237),
    unit(&["st", "stone", "stones"], "st", Mass, 6.35029318),
    // Volume (base: litre)
    unit(&["ml", "milliliter", "milliliters", "millilitre", "millilitres"], "ml", Volume, 0.001),
    unit(&["l", "liter", "liters", "litre", "litres"], "l", Volume, 1.0),
    unit(&["tsp", "teaspoon", "teaspoons"], "tsp", Volume, 0.00492892159375),
    unit(&["tbsp", "tablespoon", "tablespoons"], "tbsp", Volume, 0.01478676478125),
    unit(&["floz", "fl oz"], "fl oz", Volume, 0.0295735295625),
    unit(&["cup", "cups"], "cup", Volume, 0.2365882365),
    unit(&["pt", "pint", "pints"], "pt", Volume, 0.473176473),
    unit(&["qt", "quart", "quarts"], "qt", Volume, 0.946352946),
    unit(&["gal", "gallon", "gallons"], "gal", Volume, 3.785411784),
    // Time (base: second)
    unit(&["ms", "millisecond", "milliseconds"], "ms", Time, 0.001),
    unit(&["s", "sec", "secs", "second", "seconds"], "s", Time, 1.0),
    unit(&["min", "mins", "minute", "minutes"], "min", Time, 60.0),
    unit(&["h", "hr", "hrs", "hour", "hours"], "h", Time, 3600.0),
    unit(&["d", "day", "days"], "d", Time, 86400.0),
    unit(&["wk", "week", "weeks"], "wk", Time, 604800.0),
    unit(&["yr", "year", "years"], "yr", Time, 31_557_600.0),
    // Data (base: byte; decimal SI and binary IEC)
    unit(&["b", "byte", "bytes"], "B", Data, 1.0),
    unit(&["kb", "kilobyte", "kilobytes"], "KB", Data, 1e3),
    unit(&["mb", "megabyte", "megabytes"], "MB", Data, 1e6),
    unit(&["gb", "gigabyte", "gigabytes"], "GB", Data, 1e9),
    unit(&["tb", "terabyte", "terabytes"], "TB", Data, 1e12),
    unit(&["kib", "kibibyte", "kibibytes"], "KiB", Data, 1024.0),
    unit(&["mib", "mebibyte", "mebibytes"], "MiB", Data, 1_048_576.0),
    unit(&["gib", "gibibyte", "gibibytes"], "GiB", Data, 1_073_741_824.0),
    unit(&["tib", "tebibyte", "tebibytes"], "TiB", Data, 1_099_511_627_776.0),
    // Speed (base: metre per second)
    unit(&["m/s", "mps"], "m/s", Speed, 1.0),
    unit(&["km/h", "kph", "kmh", "kmph"], "km/h", Speed, 1.0 / 3.6),
    unit(&["mph", "mi/h"], "mph", Speed, 0.44704),
    unit(&["kn", "knot", "knots"], "kn", Speed, 0.514444),
    // Area (base: square metre)
    unit(&["m2", "sqm", "m^2"], "m²", Area, 1.0),
    unit(&["km2", "sqkm", "km^2"], "km²", Area, 1e6),
    unit(&["ft2", "sqft", "ft^2"], "ft²", Area, 0.09290304),
    unit(&["ha", "hectare", "hectares"], "ha", Area, 10_000.0),
    unit(&["ac", "acre", "acres"], "ac", Area, 4046.8564224),
    // Temperature (special-cased)
    unit(&["c", "°c", "celsius", "degc"], "°C", Temperature, 1.0),
    unit(&["f", "°f", "fahrenheit", "degf"], "°F", Temperature, 1.0),
    unit(&["k", "kelvin"], "K", Temperature, 1.0),
];

fn find_unit(text: &str) -> Option<&'static Unit> {
    let text = text.trim().to_lowercase();

    UNITS.iter().find(|unit| unit.names.contains(&text.as_str()))
}

fn to_celsius(value: f64, unit: &Unit) -> f64 {
    match unit.symbol {
        "°F" => (value - 32.0) * 5.0 / 9.0,
        "K" => value - 273.15,
        _ => value,
    }
}

fn from_celsius(value: f64, unit: &Unit) -> f64 {
    match unit.symbol {
        "°F" => value * 9.0 / 5.0 + 32.0,
        "K" => value + 273.15,
        _ => value,
    }
}

/// Convert `value` between two units of the same category.
fn convert(value: f64, from: &Unit, to: &Unit) -> Option<f64> {
    if from.category != to.category {
        return None;
    }

    Some(if from.category == Temperature {
        from_celsius(to_celsius(value, from), to)
    } else {
        value * from.factor / to.factor
    })
}

/// Format without scientific noise: up to 6 decimals, trailing zeros removed.
pub fn format_number(value: f64) -> String {
    if value != 0.0 && (value.abs() >= 1e15 || value.abs() < 1e-6) {
        return format!("{value:.4e}");
    }

    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');

    if text.is_empty() || text == "-" || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn split_number(text: &str) -> Option<(f64, &str)> {
    let text = text.trim();
    let end = text
        .char_indices()
        .find(|(index, c)| !(c.is_ascii_digit() || *c == '.' || (*index == 0 && *c == '-')))
        .map_or(text.len(), |(index, _)| index);

    let number: f64 = text[..end].parse().ok()?;

    Some((number, text[end..].trim()))
}

/// Parse `"<number> <unit> to <unit>"` and return `(answer, original)`.
pub fn convert_query(query: &str) -> Option<(String, String)> {
    let lower = query.trim().to_lowercase();

    let (left, right) = [" to ", " in ", " as ", "->", "=>"]
        .iter()
        .find_map(|separator| lower.split_once(separator))?;

    let (value, from_text) = split_number(left)?;
    let from = find_unit(from_text)?;
    let to = find_unit(right)?;
    let answer = convert(value, from, to)?;

    Some((
        format!("{} {}", format_number(answer), to.symbol),
        format!("{} {}", format_number(value), from.symbol),
    ))
}

/// Answers conversion queries such as `10 km to mi`.
pub struct UnitsProvider;

impl Provider for UnitsProvider {
    fn id(&self) -> &'static str {
        "units"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let Some((answer, original)) = convert_query(query) else {
            return Vec::new();
        };

        vec![SearchResult {
            id: format!("units:{answer}"),
            title: answer,
            subtitle: Some(format!("= {original}")),
            kind: ResultKind::Calculation,
            score: 1900,
            icon: None,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(query: &str) -> Option<String> {
        convert_query(query).map(|(answer, _)| answer)
    }

    #[test]
    fn converts_length() {
        assert_eq!(answer("10 km to mi").as_deref(), Some("6.213712 mi"));
        assert_eq!(answer("1 mi in m").as_deref(), Some("1609.344 m"));
    }

    #[test]
    fn converts_mass_and_data() {
        assert_eq!(answer("1 kg to g").as_deref(), Some("1000 g"));
        assert_eq!(answer("2 gb to mb").as_deref(), Some("2000 MB"));
        assert_eq!(answer("1 gib to mib").as_deref(), Some("1024 MiB"));
    }

    #[test]
    fn converts_temperature() {
        assert_eq!(answer("100 c to f").as_deref(), Some("212 °F"));
        assert_eq!(answer("32 f to c").as_deref(), Some("0 °C"));
        assert_eq!(answer("0 c to k").as_deref(), Some("273.15 K"));
    }

    #[test]
    fn rejects_mixed_categories_and_garbage() {
        assert_eq!(answer("5 kg to km"), None);
        assert_eq!(answer("hello to world"), None);
        assert_eq!(answer("10 km"), None);
    }

    #[test]
    fn accepts_attached_units_and_negative_numbers() {
        assert_eq!(answer("10km to m").as_deref(), Some("10000 m"));
        assert_eq!(answer("-40 c to f").as_deref(), Some("-40 °F"));
    }

    #[test]
    fn formats_numbers_compactly() {
        assert_eq!(format_number(2.50), "2.5");
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(-0.0), "0");
    }
}
