use crate::model::{Provider, ResultKind, SearchResult};

const MAX_INPUT_LEN: usize = 256;

/// Evaluate a simple arithmetic expression: `+ - * / % ^`, unary minus, parentheses.
pub fn evaluate(input: &str) -> Option<f64> {
    let input = input.trim();

    if input.is_empty() || input.len() > MAX_INPUT_LEN {
        return None;
    }

    let mut parser = Parser::new(input);
    let value = parser.expr()?;

    if parser.peek().is_some() {
        return None;
    }

    value.is_finite().then_some(value)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&mut self) -> Option<char> {
        while self.chars.get(self.pos).is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }

        self.chars.get(self.pos).copied()
    }

    fn expr(&mut self) -> Option<f64> {
        let mut value = self.term()?;

        while let Some(op) = self.peek() {
            match op {
                '+' => {
                    self.pos += 1;
                    value += self.term()?;
                }
                '-' => {
                    self.pos += 1;
                    value -= self.term()?;
                }
                _ => break,
            }
        }

        Some(value)
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.unary()?;

        while let Some(op) = self.peek() {
            match op {
                '*' => {
                    self.pos += 1;
                    value *= self.unary()?;
                }
                '/' => {
                    self.pos += 1;
                    value /= self.unary()?;
                }
                '%' => {
                    self.pos += 1;
                    value %= self.unary()?;
                }
                _ => break,
            }
        }

        Some(value)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek()? {
            '-' => {
                self.pos += 1;
                self.unary().map(|value| -value)
            }
            '+' => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.atom()?;

        if self.peek() == Some('^') {
            self.pos += 1;
            let exponent = self.unary()?;
            Some(base.powf(exponent))
        } else {
            Some(base)
        }
    }

    fn atom(&mut self) -> Option<f64> {
        match self.peek()? {
            '(' => {
                self.pos += 1;
                let value = self.expr()?;

                if self.peek() == Some(')') {
                    self.pos += 1;
                    Some(value)
                } else {
                    None
                }
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = self.pos;

                while self
                    .chars
                    .get(self.pos)
                    .is_some_and(|c| c.is_ascii_digit() || *c == '.')
                {
                    self.pos += 1;
                }

                self.chars[start..self.pos]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .ok()
            }
            _ => None,
        }
    }
}

/// Answers arithmetic queries such as `2*(3+4)`.
pub struct CalculatorProvider;

impl Provider for CalculatorProvider {
    fn id(&self) -> &'static str {
        "calc"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let query = query.trim();

        // Require an operator so plain numbers do not produce a "calculation".
        let has_operator = query
            .chars()
            .skip(1)
            .any(|c| matches!(c, '+' | '-' | '*' | '/' | '^' | '%'));

        if !has_operator {
            return Vec::new();
        }

        let Some(value) = evaluate(query) else {
            return Vec::new();
        };

        let rounded = (value * 1e10).round() / 1e10;

        vec![SearchResult {
            id: format!("calc:{rounded}"),
            title: rounded.to_string(),
            subtitle: Some(format!("= {query}")),
            kind: ResultKind::Calculation,
            score: 2000,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_parentheses() {
        assert_eq!(evaluate("2+3*4"), Some(14.0));
        assert_eq!(evaluate("(2+3)*4"), Some(20.0));
        assert_eq!(evaluate("2^3^2"), Some(512.0));
        assert_eq!(evaluate("-3 + 5"), Some(2.0));
        assert_eq!(evaluate("10 % 4"), Some(2.0));
    }

    #[test]
    fn rejects_invalid_input() {
        assert_eq!(evaluate("2+"), None);
        assert_eq!(evaluate("(2+3"), None);
        assert_eq!(evaluate("1/0"), None);
        assert_eq!(evaluate("abc"), None);
        assert_eq!(evaluate(""), None);
    }

    #[test]
    fn provider_ignores_plain_numbers() {
        assert!(CalculatorProvider.search("42").is_empty());
        assert!(CalculatorProvider.search("-5").is_empty());
    }

    #[test]
    fn provider_rounds_float_noise() {
        let results = CalculatorProvider.search("0.1+0.2");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "0.3");
    }
}
