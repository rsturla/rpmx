//! A small PEP 508 environment-marker evaluator.
//!
//! `Requires-Dist` entries may be conditional (`; python_version < "3.9"` or
//! `; extra == "test"`). The dist-info generator evaluates the common
//! conditions against the interpreter and target it is packaging for and
//! returns `Unknown` for anything it does not model, so the caller can decide
//! whether to keep the dependency.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tristate {
    True,
    False,
    Unknown,
}

pub struct Environment<'a> {
    pub python_version: &'a str,
    pub platform_machine: &'a str,
    pub extra: &'a str,
}

impl Environment<'_> {
    fn get(&self, name: &str) -> Option<&str> {
        match name {
            "python_version" | "python_full_version" => Some(self.python_version),
            "sys_platform" => Some("linux"),
            "platform_system" => Some("Linux"),
            "platform_machine" => Some(self.platform_machine),
            "os_name" => Some("posix"),
            "implementation_name" => Some("cpython"),
            "platform_python_implementation" => Some("CPython"),
            "extra" => Some(self.extra),
            _ => None,
        }
    }
}

pub fn evaluate(marker: &str, environment: &Environment<'_>) -> Tristate {
    let Some(tokens) = tokenize(marker) else {
        return Tristate::Unknown;
    };
    let mut parser = Parser {
        tokens: &tokens,
        position: 0,
        environment,
    };
    let value = parser.parse_or();
    if parser.position == tokens.len() {
        value
    } else {
        Tristate::Unknown
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Ident(String),
    String(String),
    Operator(String),
    And,
    Or,
    In,
    Not,
    LeftParen,
    RightParen,
}

fn tokenize(marker: &str) -> Option<Vec<Token>> {
    let characters = marker.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character.is_whitespace() {
            index += 1;
            continue;
        }
        match character {
            '(' => {
                tokens.push(Token::LeftParen);
                index += 1;
            }
            ')' => {
                tokens.push(Token::RightParen);
                index += 1;
            }
            '\'' | '"' => {
                let quote = character;
                index += 1;
                let mut value = String::new();
                let mut closed = false;
                while index < characters.len() {
                    let character = characters[index];
                    index += 1;
                    if character == '\\' && index < characters.len() {
                        value.push(characters[index]);
                        index += 1;
                    } else if character == quote {
                        closed = true;
                        break;
                    } else {
                        value.push(character);
                    }
                }
                if !closed {
                    return None;
                }
                tokens.push(Token::String(value));
            }
            '=' | '<' | '>' | '!' | '~' => {
                let start = index;
                index += 1;
                while index < characters.len()
                    && matches!(characters[index], '=' | '<' | '>' | '!' | '~')
                {
                    index += 1;
                }
                tokens.push(Token::Operator(characters[start..index].iter().collect()));
            }
            _ if character.is_ascii_alphabetic() || character == '_' => {
                let start = index;
                index += 1;
                while index < characters.len()
                    && (characters[index].is_ascii_alphanumeric()
                        || matches!(characters[index], '_' | '.'))
                {
                    index += 1;
                }
                let word = characters[start..index].iter().collect::<String>();
                tokens.push(match word.as_str() {
                    "and" => Token::And,
                    "or" => Token::Or,
                    "in" => Token::In,
                    "not" => Token::Not,
                    _ => Token::Ident(word),
                });
            }
            _ => return None,
        }
    }
    Some(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
    environment: &'a Environment<'a>,
}

impl Parser<'_> {
    fn parse_or(&mut self) -> Tristate {
        let mut value = self.parse_and();
        while self.eat(&Token::Or) {
            value = or(value, self.parse_and());
        }
        value
    }

    fn parse_and(&mut self) -> Tristate {
        let mut value = self.parse_comparison();
        while self.eat(&Token::And) {
            value = and(value, self.parse_comparison());
        }
        value
    }

    fn parse_comparison(&mut self) -> Tristate {
        if self.eat(&Token::LeftParen) {
            let value = self.parse_or();
            if !self.eat(&Token::RightParen) {
                return Tristate::Unknown;
            }
            return value;
        }
        let left = self.parse_value();
        let operator = match self.next().cloned() {
            Some(Token::Operator(operator)) => operator,
            Some(Token::In) => "in".to_string(),
            Some(Token::Not) if self.eat(&Token::In) => "not in".to_string(),
            _ => return Tristate::Unknown,
        };
        let right = self.parse_value();
        compare(&left, &operator, &right)
    }

    fn parse_value(&mut self) -> Value {
        match self.next().cloned() {
            Some(Token::Ident(name)) => match self.environment.get(&name) {
                Some(value) => Value::Known(value.to_string()),
                None => Value::Unknown,
            },
            Some(Token::String(value)) => Value::Known(value),
            _ => Value::Unknown,
        }
    }

    fn next(&mut self) -> Option<&Token> {
        let token = self.tokens.get(self.position)?;
        self.position += 1;
        Some(token)
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.tokens.get(self.position) == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }
}

enum Value {
    Known(String),
    Unknown,
}

fn compare(left: &Value, operator: &str, right: &Value) -> Tristate {
    let (Value::Known(left), Value::Known(right)) = (left, right) else {
        return Tristate::Unknown;
    };
    let result = match operator {
        "in" => right.contains(left.as_str()),
        "not in" => !right.contains(left.as_str()),
        "==" | "===" => equal(left, right),
        "!=" => !equal(left, right),
        "<" => ordering(left, right).is_some_and(|ordering| ordering.is_lt()),
        "<=" => ordering(left, right).is_some_and(|ordering| ordering.is_le()),
        ">" => ordering(left, right).is_some_and(|ordering| ordering.is_gt()),
        ">=" => ordering(left, right).is_some_and(|ordering| ordering.is_ge()),
        _ => return Tristate::Unknown,
    };
    if result {
        Tristate::True
    } else {
        Tristate::False
    }
}

fn equal(left: &str, right: &str) -> bool {
    match (numbers(left), numbers(right)) {
        (Some(left), Some(right)) => trim_zeros(&left) == trim_zeros(&right),
        _ => left == right,
    }
}

fn ordering(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    match (numbers(left), numbers(right)) {
        (Some(left), Some(right)) => Some(trim_zeros(&left).cmp(trim_zeros(&right))),
        _ => Some(left.cmp(right)),
    }
}

fn numbers(value: &str) -> Option<Vec<u64>> {
    let value = value.strip_prefix(['v', 'V']).unwrap_or(value);
    if value.is_empty() {
        return None;
    }
    value
        .split('.')
        .map(|component| component.parse::<u64>().ok())
        .collect()
}

fn trim_zeros(mut numbers: &[u64]) -> &[u64] {
    while numbers.len() > 1 && numbers.last() == Some(&0) {
        numbers = &numbers[..numbers.len() - 1];
    }
    numbers
}

fn or(left: Tristate, right: Tristate) -> Tristate {
    match (left, right) {
        (Tristate::True, _) | (_, Tristate::True) => Tristate::True,
        (Tristate::Unknown, _) | (_, Tristate::Unknown) => Tristate::Unknown,
        _ => Tristate::False,
    }
}

fn and(left: Tristate, right: Tristate) -> Tristate {
    match (left, right) {
        (Tristate::False, _) | (_, Tristate::False) => Tristate::False,
        (Tristate::Unknown, _) | (_, Tristate::Unknown) => Tristate::Unknown,
        _ => Tristate::True,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluate(marker: &str) -> Tristate {
        let environment = Environment {
            python_version: "3.12",
            platform_machine: "x86_64",
            extra: "",
        };
        super::evaluate(marker, &environment)
    }

    #[test]
    fn python_version_comparisons_use_numeric_ordering() {
        assert_eq!(evaluate("python_version >= \"3.8\""), Tristate::True);
        assert_eq!(evaluate("python_version < \"3.8\""), Tristate::False);
        assert_eq!(evaluate("python_version < \"3.10\""), Tristate::False);
    }

    #[test]
    fn extras_are_not_part_of_a_base_install() {
        assert_eq!(evaluate("extra == \"test\""), Tristate::False);
        assert_eq!(evaluate("extra != \"test\""), Tristate::True);
    }

    #[test]
    fn platform_markers_are_evaluated() {
        assert_eq!(evaluate("sys_platform == \"win32\""), Tristate::False);
        assert_eq!(evaluate("sys_platform == \"linux\""), Tristate::True);
        assert_eq!(evaluate("platform_machine == \"x86_64\""), Tristate::True);
    }

    #[test]
    fn boolean_expressions_and_parentheses_work() {
        assert_eq!(
            evaluate("python_version >= \"3.8\" and extra == \"test\""),
            Tristate::False
        );
        assert_eq!(
            evaluate("(python_version < \"3.8\" or python_version >= \"3.10\") and sys_platform == \"linux\""),
            Tristate::True
        );
        assert_eq!(
            evaluate("python_version < \"3.8\" or sys_platform == \"win32\""),
            Tristate::False
        );
    }

    #[test]
    fn membership_tests_work() {
        assert_eq!(
            evaluate("python_version in \"3.10 3.11 3.12\""),
            Tristate::True
        );
        assert_eq!(
            evaluate("python_version not in \"3.10 3.11\""),
            Tristate::True
        );
    }

    #[test]
    fn unknown_variables_and_syntax_return_unknown() {
        assert_eq!(evaluate("platform_release >= \"5\""), Tristate::Unknown);
        assert_eq!(evaluate("python_version >="), Tristate::Unknown);
        assert_eq!(evaluate("not a marker"), Tristate::Unknown);
    }
}
