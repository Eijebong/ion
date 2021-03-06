// TODO:
// - Rewrite this in the same style as shell_expand::words.
// - Validate syntax in methods

use std::fmt::{self, Display, Formatter};
use std::u16;

bitflags! {
    pub struct Flags : u16 {
        const DQUOTE = 1;
        const COMM_1 = 2;
        const COMM_2 = 4;
        const VBRACE = 8;
        const ARRAY  = 16;
        const VARIAB = 32;
        const METHOD = 64;
        /// Set while parsing through an inline arithmetic expression, e.g. $((foo * bar / baz))
        const MATHEXPR = 128;
        const POST_MATHEXPR = 256;
    }
}


#[derive(Debug, PartialEq)]
pub(crate) enum StatementError<'a> {
    IllegalCommandName(&'a str),
    InvalidCharacter(char, usize),
    UnterminatedSubshell,
    UnterminatedBracedVar,
    UnterminatedBrace,
    UnterminatedMethod,
    UnterminatedArithmetic,
    ExpectedCommandButFound(&'static str),
}

impl<'a> Display for StatementError<'a> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match *self {
            StatementError::IllegalCommandName(command) => {
                writeln!(f, "illegal command name: {}", command)
            }
            StatementError::InvalidCharacter(character, position) => writeln!(
                f,
                "syntax error: '{}' at position {} is out of place",
                character,
                position
            ),
            StatementError::UnterminatedSubshell => {
                writeln!(f, "syntax error: unterminated subshell")
            }
            StatementError::UnterminatedBrace => writeln!(f, "syntax error: unterminated brace"),
            StatementError::UnterminatedBracedVar => {
                writeln!(f, "syntax error: unterminated braced var")
            }
            StatementError::UnterminatedMethod => writeln!(f, "syntax error: unterminated method"),
            StatementError::UnterminatedArithmetic => {
                writeln!(f, "syntax error: unterminated arithmetic subexpression")
            }
            StatementError::ExpectedCommandButFound(element) => {
                writeln!(f, "expected command, but found {}", element)
            }
        }
    }
}

/// Returns true if the byte matches [^A-Za-z0-9_]
fn is_invalid(byte: u8) -> bool {
    byte <= 47 || (byte >= 58 && byte <= 64) || (byte >= 91 && byte <= 94) || byte == 96
        || (byte >= 123 && byte <= 127)
}

pub(crate) struct StatementSplitter<'a> {
    data:             &'a str,
    read:             usize,
    flags:            Flags,
    a_level:          u8,
    ap_level:         u8,
    p_level:          u8,
    brace_level:      u8,
    math_paren_level: i8,
}

impl<'a> StatementSplitter<'a> {
    pub(crate) fn new(data: &'a str) -> StatementSplitter<'a> {
        StatementSplitter {
            data:             data,
            read:             0,
            flags:            Flags::empty(),
            a_level:          0,
            ap_level:         0,
            p_level:          0,
            brace_level:      0,
            math_paren_level: 0,
        }
    }

    fn single_quote<B: Iterator<Item = u8>>(&mut self, bytes: &mut B) -> usize {
        let mut read = 0;
        while let Some(character) = bytes.next() {
            read += 1;
            if character == b'\\' {
                read += 1;
                bytes.next();
            } else if character == b'\'' {
                break;
            }
        }
        read
    }
}

impl<'a> Iterator for StatementSplitter<'a> {
    type Item = Result<&'a str, StatementError<'a>>;
    fn next(&mut self) -> Option<Result<&'a str, StatementError<'a>>> {
        let start = self.read;
        let mut first_arg_found = false;
        let mut else_found = false;
        let mut else_pos = 0;
        let mut error = None;
        let mut bytes = self.data.bytes().skip(self.read);
        while let Some(character) = bytes.next() {
            self.read += 1;
            match character {
                b'\\' => {
                    self.read += 1;
                    bytes.next();
                }
                _ if self.flags.contains(POST_MATHEXPR) => {
                    self.flags -= POST_MATHEXPR;
                }
                // [^A-Za-z0-9_:,}]
                0...43 | 45...47 | 59...64 | 91...94 | 96 | 123...124 | 126...127
                    if self.flags.contains(VBRACE) =>
                {
                    // If we are just ending the braced section continue as normal
                    if error.is_none() {
                        error = Some(StatementError::InvalidCharacter(character as char, self.read))
                    }
                }
                b'\'' if !self.flags.contains(DQUOTE) => {
                    self.flags -= VARIAB | ARRAY;
                    self.read += self.single_quote(&mut bytes);
                }
                // Toggle DQUOTE and disable VARIAB + ARRAY.
                b'"' => self.flags = (self.flags ^ DQUOTE) - (VARIAB | ARRAY),
                // Disable COMM_1 and enable COMM_2 + ARRAY.
                b'@' => {
                    self.flags = (self.flags - COMM_1) | (COMM_2 | ARRAY);
                    continue;
                }
                b'$' => {
                    self.flags = (self.flags - COMM_2) | (COMM_1 | VARIAB);
                    continue;
                }
                b'{' if self.flags.intersects(COMM_1 | COMM_2) => self.flags |= VBRACE,
                b'{' if !self.flags.contains(DQUOTE) => self.brace_level += 1,
                b'}' if self.flags.contains(VBRACE) => self.flags.toggle(VBRACE),
                b'}' if !self.flags.contains(DQUOTE) => if self.brace_level == 0 {
                    if error.is_none() {
                        error = Some(StatementError::InvalidCharacter(character as char, self.read))
                    }
                } else {
                    self.brace_level -= 1;
                },
                b'(' if self.flags.contains(MATHEXPR) => {
                    self.math_paren_level += 1;
                }
                b'(' if !self.flags.intersects(COMM_1 | VARIAB | ARRAY) => {
                    if error.is_none() && !self.flags.contains(DQUOTE) {
                        error = Some(StatementError::InvalidCharacter(character as char, self.read))
                    }
                }
                b'(' if self.flags.intersects(COMM_1 | METHOD) => {
                    self.flags -= VARIAB | ARRAY;
                    if self.data.as_bytes()[self.read] == b'(' {
                        self.flags = (self.flags - COMM_1) | MATHEXPR;
                        // The next character will always be a left paren in this branch;
                        self.math_paren_level = -1;
                    } else {
                        self.p_level += 1;
                    }
                }
                b'(' if self.flags.contains(COMM_2) => {
                    self.ap_level += 1;
                }
                b'(' if self.flags.intersects(VARIAB | ARRAY) => {
                    self.flags = (self.flags - (VARIAB | ARRAY)) | METHOD;
                }
                b')' if self.flags.contains(MATHEXPR) => if self.math_paren_level == 0 {
                    if self.data.as_bytes().len() <= self.read {
                        if error.is_none() {
                            error = Some(StatementError::UnterminatedArithmetic)
                        }
                    } else {
                        let next_character = self.data.as_bytes()[self.read] as char;
                        if next_character == ')' {
                            self.flags = (self.flags - MATHEXPR) | POST_MATHEXPR;
                        } else if error.is_none() {
                            error =
                                Some(StatementError::InvalidCharacter(next_character, self.read));
                        }
                    }
                } else {
                    self.math_paren_level -= 1;
                },
                b')' if self.flags.contains(METHOD) && self.p_level == 0 => {
                    self.flags ^= METHOD;
                }
                b')' if self.p_level + self.ap_level == 0 => {
                    if error.is_none() && !self.flags.contains(DQUOTE) {
                        error = Some(StatementError::InvalidCharacter(character as char, self.read))
                    }
                }
                b')' if self.p_level != 0 => self.p_level -= 1,
                b')' => self.ap_level -= 1,
                b';' if !self.flags.contains(DQUOTE) && self.p_level == 0 && self.ap_level == 0 => {
                    return match error {
                        Some(error) => Some(Err(error)),
                        None => Some(Ok(self.data[start..self.read - 1].trim())),
                    }
                }
                b'#' if self.read == 1
                    || (!self.flags.contains(DQUOTE) && self.p_level + self.ap_level == 0
                        && match self.data.as_bytes()[self.read - 2] {
                            b' ' | b'\t' => true,
                            _ => false,
                        }) =>
                {
                    let output = self.data[start..self.read - 1].trim();
                    self.read = self.data.len();
                    return match error {
                        Some(error) => Some(Err(error)),
                        None => Some(Ok(output)),
                    };
                }
                b' ' if else_found => {
                    let output = &self.data[else_pos..self.read - 1].trim();
                    if !output.is_empty() {
                        if "if" != *output {
                            self.read = else_pos;
                            return Some(Ok("else"));
                        }
                    }
                    else_found = false;
                }
                b' ' if !first_arg_found => {
                    let output = &self.data[start..self.read - 1].trim();
                    if !output.is_empty() {
                        match *output {
                            "else" => {
                                else_found = true;
                                else_pos = self.read;
                            }
                            _ => first_arg_found = true,
                        }
                    }
                }
                // [^A-Za-z0-9_]
                byte => if self.flags.intersects(VARIAB | ARRAY) {
                    self.flags -= if is_invalid(byte) { VARIAB | ARRAY } else { Flags::empty() };
                },
            }
            self.flags -= COMM_1 | COMM_2;
        }

        if start == self.read {
            None
        } else {
            self.read = self.data.len();
            match error {
                Some(error) => Some(Err(error)),
                None if self.p_level != 0 || self.ap_level != 0 || self.a_level != 0 => {
                    Some(Err(StatementError::UnterminatedSubshell))
                }
                None if self.flags.contains(METHOD) => {
                    Some(Err(StatementError::UnterminatedMethod))
                }
                None if self.flags.contains(VBRACE) => {
                    Some(Err(StatementError::UnterminatedBracedVar))
                }
                None if self.brace_level != 0 => Some(Err(StatementError::UnterminatedBrace)),
                None if self.flags.contains(MATHEXPR) => {
                    Some(Err(StatementError::UnterminatedArithmetic))
                }
                None => {
                    let output = self.data[start..].trim();
                    if output.is_empty() {
                        return Some(Ok(output));
                    }
                    match output.as_bytes()[0] {
                        b'>' | b'<' | b'^' => {
                            Some(Err(StatementError::ExpectedCommandButFound("redirection")))
                        }
                        b'|' => Some(Err(StatementError::ExpectedCommandButFound("pipe"))),
                        b'&' => Some(Err(StatementError::ExpectedCommandButFound("&"))),
                        b'*' | b'%' | b'?' | b'{' | b'}' => {
                            Some(Err(StatementError::IllegalCommandName(output)))
                        }
                        _ => Some(Ok(output)),
                    }
                }
            }
        }
    }
}

#[test]
fn syntax_errors() {
    let command = "echo (echo one); echo $( (echo one); echo ) two; echo $(echo one";
    let results = StatementSplitter::new(command).collect::<Vec<Result<&str, StatementError>>>();
    assert_eq!(results[0], Err(StatementError::InvalidCharacter('(', 6)));
    assert_eq!(results[1], Err(StatementError::InvalidCharacter('(', 26)));
    assert_eq!(results[2], Err(StatementError::InvalidCharacter(')', 43)));
    assert_eq!(results[3], Err(StatementError::UnterminatedSubshell));
    assert_eq!(results.len(), 4);

    let command = ">echo";
    let results = StatementSplitter::new(command).collect::<Vec<Result<&str, StatementError>>>();
    assert_eq!(results[0], Err(StatementError::ExpectedCommandButFound("redirection")));
    assert_eq!(results.len(), 1);

    let command = "echo $((foo bar baz)";
    let results = StatementSplitter::new(command).collect::<Vec<_>>();
    assert_eq!(results[0], Err(StatementError::UnterminatedArithmetic));
    assert_eq!(results.len(), 1);
}

#[test]
fn methods() {
    let command = "echo $join(array, ', '); echo @join(var, ', ')";
    let statements = StatementSplitter::new(command).collect::<Vec<_>>();
    assert_eq!(statements[0], Ok("echo $join(array, ', ')"));
    assert_eq!(statements[1], Ok("echo @join(var, ', ')"));
    assert_eq!(statements.len(), 2);
}

#[test]
fn processes() {
    let command = "echo $(seq 1 10); echo $(seq 1 10)";
    for statement in StatementSplitter::new(command) {
        assert_eq!(statement, Ok("echo $(seq 1 10)"));
    }
}

#[test]
fn array_processes() {
    let command = "echo @(echo one; sleep 1); echo @(echo one; sleep 1)";
    for statement in StatementSplitter::new(command) {
        assert_eq!(statement, Ok("echo @(echo one; sleep 1)"));
    }
}

#[test]
fn process_with_statements() {
    let command = "echo $(seq 1 10; seq 1 10)";
    for statement in StatementSplitter::new(command) {
        assert_eq!(statement, Ok(command));
    }
}

#[test]
fn quotes() {
    let command = "echo \"This ;'is a test\"; echo 'This ;\" is also a test'";
    let results = StatementSplitter::new(command).collect::<Vec<Result<&str, StatementError>>>();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0], Ok("echo \"This ;'is a test\""));
    assert_eq!(results[1], Ok("echo 'This ;\" is also a test'"));
}

#[test]
fn comments() {
    let command = "echo $(echo one # two); echo three # four";
    let results = StatementSplitter::new(command).collect::<Vec<Result<&str, StatementError>>>();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0], Ok("echo $(echo one # two)"));
    assert_eq!(results[1], Ok("echo three"));
}

#[test]
fn nested_process() {
    let command = "echo $(echo one $(echo two) three)";
    let results = StatementSplitter::new(command).collect::<Vec<Result<&str, StatementError>>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], Ok(command));

    let command = "echo $(echo $(echo one; echo two); echo two)";
    let results = StatementSplitter::new(command).collect::<Vec<Result<&str, StatementError>>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], Ok(command));
}

#[test]
fn nested_array_process() {
    let command = "echo @(echo one @(echo two) three)";
    let results = StatementSplitter::new(command).collect::<Vec<Result<&str, StatementError>>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], Ok(command));

    let command = "echo @(echo @(echo one; echo two); echo two)";
    let results = StatementSplitter::new(command).collect::<Vec<Result<&str, StatementError>>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], Ok(command));
}

#[test]
fn braced_variables() {
    let command = "echo ${foo}bar ${bar}baz ${baz}quux @{zardoz}wibble";
    let results = StatementSplitter::new(command).collect::<Vec<Result<&str, StatementError>>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results, vec![Ok(command)]);
}
