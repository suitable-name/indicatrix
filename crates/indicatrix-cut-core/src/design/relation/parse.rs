//! The arithmetic text reader: a lexer and a recursive-descent parser that turn text such
//! as `P1 - [Crown Main] + @3` into an [`Expr`] whose names are still unresolved.
//!
//! Only [`Expr::parse_syntax`] and `is_bare_name` are used outside this module.

use super::{Expr, ExprError, MAX_NESTING, MAX_NODES, RawName};

// --- lexing -------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Name(RawName),
    Plus,
    Minus,
    Star,
    Slash,
    Open,
    Close,
}

impl Token {
    /// How a token reads in a message.
    fn describe(&self) -> String {
        match self {
            Self::Number(number) => format!("the number {number}"),
            Self::Name(name) => format!("'{}'", name.text()),
            Self::Plus => "'+'".to_owned(),
            Self::Minus => "'-'".to_owned(),
            Self::Star => "'*'".to_owned(),
            Self::Slash => "'/'".to_owned(),
            Self::Open => "'('".to_owned(),
            Self::Close => "')'".to_owned(),
        }
    }
}

fn syntax(message: impl Into<String>) -> ExprError {
    ExprError::Syntax(message.into())
}

/// Whether `c` may start a bare name.
fn starts_bare_name(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

/// Whether `c` may continue a bare name.
fn continues_bare_name(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `name` can be written without brackets.
pub(super) fn is_bare_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(starts_bare_name) && chars.all(continues_bare_name)
}

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    const fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn take_while(&mut self, keep: impl Fn(char) -> bool) -> &'a str {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if !keep(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
        &self.text[start..self.pos]
    }

    /// Whether the text after the current `e`/`E` is an exponent (digits, optionally
    /// signed). A name such as `e1` after a number is then not swallowed by it.
    fn exponent_follows(&self) -> bool {
        let mut rest = self.text[self.pos..].chars().skip(1);
        match rest.next() {
            Some(c) if c.is_ascii_digit() => true,
            Some('+' | '-') => rest.next().is_some_and(|c| c.is_ascii_digit()),
            _ => false,
        }
    }

    fn number(&mut self) -> Result<f64, ExprError> {
        let start = self.pos;
        self.take_while(|c| c.is_ascii_digit());
        if self.peek() == Some('.') {
            self.bump();
            self.take_while(|c| c.is_ascii_digit());
        }
        if matches!(self.peek(), Some('e' | 'E')) && self.exponent_follows() {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            self.take_while(|c| c.is_ascii_digit());
        }
        let literal = &self.text[start..self.pos];
        literal
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| syntax(format!("'{literal}' is not a usable number")))
    }

    fn name_in_brackets(&mut self) -> Result<RawName, ExprError> {
        self.bump();
        let inner = self.take_while(|c| c != ']');
        if self.bump() != Some(']') {
            return Err(syntax("a '[' is never closed"));
        }
        let name = inner.trim();
        if name.is_empty() {
            return Err(syntax("there is no tier name between '[' and ']'"));
        }
        Ok(RawName::Bracketed(name.to_owned()))
    }

    fn id(&mut self) -> Result<RawName, ExprError> {
        self.bump();
        let digits = self.take_while(|c| c.is_ascii_digit());
        digits
            .parse::<u64>()
            .map(RawName::Id)
            .map_err(|_| syntax("'@' must be followed by a tier number, for example @3"))
    }

    fn next_token(&mut self) -> Result<Option<Token>, ExprError> {
        self.take_while(char::is_whitespace);
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        let operator = |lexer: &mut Self, token: Token| -> Result<Option<Token>, ExprError> {
            lexer.bump();
            Ok(Some(token))
        };
        match c {
            '+' => operator(self, Token::Plus),
            '-' => operator(self, Token::Minus),
            '*' => operator(self, Token::Star),
            '/' => operator(self, Token::Slash),
            '(' => operator(self, Token::Open),
            ')' => operator(self, Token::Close),
            '[' => self.name_in_brackets().map(|name| Some(Token::Name(name))),
            '@' => self.id().map(|name| Some(Token::Name(name))),
            c if c.is_ascii_digit() => self.number().map(|n| Some(Token::Number(n))),
            '.' if self.text[self.pos + 1..]
                .chars()
                .next()
                .is_some_and(|d| d.is_ascii_digit()) =>
            {
                self.number().map(|n| Some(Token::Number(n)))
            }
            c if starts_bare_name(c) => {
                let word = self.take_while(continues_bare_name);
                Ok(Some(Token::Name(RawName::Bare(word.to_owned()))))
            }
            other => Err(syntax(format!("'{other}' is not understood here"))),
        }
    }
}

fn tokenize(text: &str) -> Result<Vec<Token>, ExprError> {
    let mut lexer = Lexer::new(text);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token()? {
        tokens.push(token);
    }
    Ok(tokens)
}

// --- parsing ------------------------------------------------------------------------

/// A constructor of a two-operand node, as a plain function.
type BuildBinary = fn(Box<Expr<RawName>>, Box<Expr<RawName>>) -> Expr<RawName>;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    nodes: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn node(&mut self, node: Expr<RawName>) -> Result<Expr<RawName>, ExprError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(ExprError::TooBig);
        }
        Ok(node)
    }

    fn binary(
        &mut self,
        build: BuildBinary,
        left: Expr<RawName>,
        right: Expr<RawName>,
    ) -> Result<Expr<RawName>, ExprError> {
        self.node(build(Box::new(left), Box::new(right)))
    }

    /// `term (('+' | '-') term)*`, left-associative.
    fn expression(&mut self, depth: usize) -> Result<Expr<RawName>, ExprError> {
        let mut left = self.term(depth)?;
        loop {
            let build: BuildBinary = match self.peek() {
                Some(Token::Plus) => Expr::Add,
                Some(Token::Minus) => Expr::Sub,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.term(depth)?;
            left = self.binary(build, left, right)?;
        }
    }

    /// `unary (('*' | '/') unary)*`, left-associative.
    fn term(&mut self, depth: usize) -> Result<Expr<RawName>, ExprError> {
        let mut left = self.unary(depth)?;
        loop {
            let build: BuildBinary = match self.peek() {
                Some(Token::Star) => Expr::Mul,
                Some(Token::Slash) => Expr::Div,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.unary(depth)?;
            left = self.binary(build, left, right)?;
        }
    }

    /// `('-' | '+') unary | primary`.
    fn unary(&mut self, depth: usize) -> Result<Expr<RawName>, ExprError> {
        match self.peek() {
            Some(Token::Minus) => {
                self.pos += 1;
                let inner = self.nested(depth, Self::unary)?;
                self.node(Expr::Neg(Box::new(inner)))
            }
            Some(Token::Plus) => {
                self.pos += 1;
                self.nested(depth, Self::unary)
            }
            _ => self.primary(depth),
        }
    }

    /// `number | name | '(' expression ')'`.
    fn primary(&mut self, depth: usize) -> Result<Expr<RawName>, ExprError> {
        let Some(token) = self.advance() else {
            return Err(syntax("a number or tier name is missing at the end"));
        };
        match token {
            Token::Number(number) => self.node(Expr::Number(number)),
            Token::Name(name) => self.node(Expr::Ref(name)),
            Token::Open => {
                let inner = self.nested(depth, Self::expression)?;
                match self.advance() {
                    Some(Token::Close) => Ok(inner),
                    _ => Err(syntax("a '(' is never closed")),
                }
            }
            other => Err(syntax(format!(
                "a number or tier name is missing before {}",
                other.describe()
            ))),
        }
    }

    /// Runs `inner` one nesting level deeper, refusing past [`MAX_NESTING`].
    fn nested(
        &mut self,
        depth: usize,
        inner: fn(&mut Self, usize) -> Result<Expr<RawName>, ExprError>,
    ) -> Result<Expr<RawName>, ExprError> {
        if depth >= MAX_NESTING {
            return Err(ExprError::TooDeep);
        }
        inner(self, depth + 1)
    }
}

impl Expr<RawName> {
    /// Reads `text` as arithmetic over numbers and names, with `max_chars` as the
    /// length limit. The names stay unresolved ([`Self::try_map_refs`] resolves them).
    ///
    /// # Errors
    ///
    /// An [`ExprError`] describing what is wrong with the text.
    pub fn parse_syntax(text: &str, max_chars: usize) -> Result<Self, ExprError> {
        if text.chars().count() > max_chars {
            return Err(ExprError::TooLong { max: max_chars });
        }
        let tokens = tokenize(text)?;
        if tokens.is_empty() {
            return Err(ExprError::Empty);
        }
        let mut parser = Parser {
            tokens,
            pos: 0,
            nodes: 0,
        };
        let expr = parser.expression(0)?;
        match parser.peek() {
            None => Ok(expr),
            Some(Token::Close) => Err(syntax("there is a ')' without a matching '('")),
            Some(other) => Err(syntax(format!(
                "put +, -, * or / in front of {}",
                other.describe()
            ))),
        }
    }
}
