//! A JSON Schema `pattern`: the ECMAScript regular expressions Zod,
//! Valibot and ArkType export (classes, groups, alternation, greedy and
//! lazy quantifiers, anchors, word boundaries, lookahead), matched by
//! backtracking anywhere in the text, as JSON Schema says. Lookbehind and
//! backreferences are refused when the schema is read, never ignored.

#[derive(Debug)]
enum Node {
    Char(char),
    Any,
    Class(Vec<Item>, bool),
    Start,
    End,
    Bound(bool),
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Look(Box<Node>, bool),
    Rep(Box<Node>, usize, usize, bool),
}

#[derive(Debug)]
enum Item {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

/// A compiled pattern.
#[derive(Debug)]
pub struct Pattern(Node);

fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

impl Item {
    fn has(&self, c: char) -> bool {
        match *self {
            Item::Range(a, b) => (a..=b).contains(&c),
            Item::Digit(n) => c.is_ascii_digit() != n,
            Item::Word(n) => word(c) != n,
            Item::Space(n) => space(c) != n,
        }
    }
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        let hit = self.peek() == Some(c);
        self.i += usize::from(hit);
        hit
    }

    fn next(&mut self) -> Result<char, String> {
        let c = self.peek().ok_or("ends too soon")?;
        self.i += 1;
        Ok(c)
    }

    fn alt(&mut self) -> Result<Node, String> {
        let mut alts = vec![self.seq()?];
        while self.eat('|') {
            alts.push(self.seq()?);
        }
        Ok(if alts.len() == 1 { alts.remove(0) } else { Node::Alt(alts) })
    }

    fn seq(&mut self) -> Result<Node, String> {
        let mut seq = vec![];
        while let Some(c) = self.peek().filter(|c| *c != '|' && *c != ')') {
            self.i += 1;
            let atom = self.atom(c)?;
            seq.push(self.quantified(atom)?);
        }
        Ok(Node::Seq(seq))
    }

    fn atom(&mut self, c: char) -> Result<Node, String> {
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                let look = if self.eat('?') {
                    match self.next()? {
                        ':' => None,
                        '=' => Some(false),
                        '!' => Some(true),
                        '<' if !matches!(self.peek(), Some('=' | '!')) => {
                            while self.next()? != '>' {}
                            None
                        }
                        _ => return Err("lookbehind and inline flags are not supported".into()),
                    }
                } else {
                    None
                };
                let inner = self.alt()?;
                if !self.eat(')') {
                    return Err("a group is never closed".into());
                }
                match look {
                    Some(neg) => Node::Look(Box::new(inner), neg),
                    None => inner,
                }
            }
            '[' => self.class()?,
            '\\' if matches!(self.peek(), Some('b' | 'B')) => Node::Bound(self.next()? == 'B'),
            '\\' => match self.escape()? {
                Ok(c) => Node::Char(c),
                Err(Some(item)) => Node::Class(vec![item], false),
                Err(None) => return Err("unknown escape".into()),
            },
            '*' | '+' | '?' => return Err(format!("nothing to repeat before {c}")),
            c => Node::Char(c),
        })
    }

    /// A char, or a class (`\d`).
    fn escape(&mut self) -> Result<Result<char, Option<Item>>, String> {
        let c = self.next()?;
        let hex = |p: &mut Self, n: usize| -> Result<char, String> {
            let h: String = (0..n).map(|_| p.next()).collect::<Result<_, _>>()?;
            u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).ok_or_else(|| format!("bad escape \\{c}{h}"))
        };
        Ok(match c {
            'd' => Err(Some(Item::Digit(false))),
            'D' => Err(Some(Item::Digit(true))),
            'w' => Err(Some(Item::Word(false))),
            'W' => Err(Some(Item::Word(true))),
            's' => Err(Some(Item::Space(false))),
            'S' => Err(Some(Item::Space(true))),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'f' => Ok('\u{c}'),
            'v' => Ok('\u{b}'),
            '0' => Ok('\0'),
            'x' => Ok(hex(self, 2)?),
            'u' if self.eat('{') => {
                let mut h = String::new();
                while let Some(d) = self.peek().filter(|d| *d != '}') {
                    h.push(d);
                    self.i += 1;
                }
                self.eat('}');
                Ok(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).ok_or("bad escape \\u{..}")?)
            }
            'u' => Ok(hex(self, 4)?),
            '1'..='9' => return Err("backreferences are not supported".into()),
            c => Ok(c),
        })
    }

    fn class(&mut self) -> Result<Node, String> {
        let neg = self.eat('^');
        let mut items = vec![];
        loop {
            let c = self.next().map_err(|_| "a class is never closed")?;
            if c == ']' {
                return Ok(Node::Class(items, neg));
            }
            let lo = match c {
                '\\' => match self.peek() {
                    Some('b') => {
                        self.i += 1;
                        '\u{8}'
                    }
                    _ => match self.escape()? {
                        Ok(c) => c,
                        Err(Some(i)) => {
                            items.push(i);
                            continue;
                        }
                        Err(None) => return Err("unknown escape".into()),
                    },
                },
                c => c,
            };
            if self.peek() == Some('-') && self.s.get(self.i + 1).is_some_and(|c| *c != ']') {
                self.i += 1;
                let hi = match self.next()? {
                    '\\' => self.escape()?.map_err(|_| "a range ends at a class")?,
                    c => c,
                };
                if hi < lo {
                    return Err(format!("range {lo}-{hi} out of order"));
                }
                items.push(Item::Range(lo, hi));
            } else {
                items.push(Item::Range(lo, lo));
            }
        }
    }

    fn number(&mut self) -> Option<usize> {
        let at = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        self.s[at..self.i].iter().collect::<String>().parse().ok()
    }

    fn quantified(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('*') => (0, usize::MAX),
            Some('+') => (1, usize::MAX),
            Some('?') => (0, 1),
            Some('{') => {
                let at = self.i;
                self.i += 1;
                let bounds = self.number().and_then(|min| match (self.eat(','), self.eat('}')) {
                    (false, true) => Some((min, min)),
                    (true, true) => Some((min, usize::MAX)),
                    (true, false) => self.number().filter(|_| self.eat('}')).map(|max| (min, max)),
                    (false, false) => None,
                });
                match bounds {
                    // Not a quantifier: a literal `{` (as ECMAScript reads it).
                    None => {
                        self.i = at;
                        return Ok(atom);
                    }
                    Some((min, max)) if max < min => return Err(format!("{{{min},{max}}} out of order")),
                    Some(b) => {
                        self.i -= 1;
                        b
                    }
                }
            }
            _ => return Ok(atom),
        };
        if matches!(atom, Node::Start | Node::End | Node::Bound(_) | Node::Look(..)) && min > 0 {
            return Ok(atom);
        }
        self.i += 1;
        let lazy = self.eat('?');
        Ok(Node::Rep(Box::new(atom), min, max, !lazy))
    }
}

type K<'k> = &'k mut dyn FnMut(usize) -> bool;

fn go(n: &Node, s: &[char], i: usize, k: K) -> bool {
    match n {
        Node::Char(c) => s.get(i) == Some(c) && k(i + 1),
        Node::Any => s.get(i).is_some_and(|c| !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')) && k(i + 1),
        Node::Class(items, neg) => s.get(i).is_some_and(|c| items.iter().any(|it| it.has(*c)) != *neg) && k(i + 1),
        Node::Start => i == 0 && k(i),
        Node::End => i == s.len() && k(i),
        Node::Bound(neg) => {
            let at = |j: Option<usize>| j.and_then(|j| s.get(j)).is_some_and(|c| word(*c));
            (at(i.checked_sub(1)) != at(Some(i))) != *neg && k(i)
        }
        Node::Seq(v) => seq(v, s, i, k),
        Node::Alt(v) => v.iter().any(|n| go(n, s, i, k)),
        Node::Look(n, neg) => go(n, s, i, &mut |_| true) != *neg && k(i),
        Node::Rep(n, min, max, greedy) => rep(n, (*min, *max, *greedy), s, i, 0, k),
    }
}

fn seq(v: &[Node], s: &[char], i: usize, k: K) -> bool {
    match v.split_first() {
        None => k(i),
        Some((h, t)) => go(h, s, i, &mut |j| seq(t, s, j, k)),
    }
}

fn rep(n: &Node, b: (usize, usize, bool), s: &[char], i: usize, count: usize, k: K) -> bool {
    let (min, max, greedy) = b;
    // An empty match counts only toward the minimum: no endless loop.
    let more = |k: K| count < max && go(n, s, i, &mut |j| (j != i || count < min) && rep(n, b, s, j, count + 1, k));
    // Lazy: stop here first; greedy: one more first.
    if !greedy && count >= min && k(i) {
        return true;
    }
    more(k) || (greedy && count >= min && k(i))
}

impl Pattern {
    pub fn new(src: &str) -> Result<Self, String> {
        let s: Vec<char> = src.chars().collect();
        let mut p = Parser { s: &s, i: 0 };
        let n = p.alt()?;
        if p.i < s.len() {
            return Err("an unmatched )".into());
        }
        Ok(Self(n))
    }

    /// Whether it matches anywhere in `text`.
    pub fn found(&self, text: &str) -> bool {
        let s: Vec<char> = text.chars().collect();
        (0..=s.len()).any(|i| go(&self.0, &s, i, &mut |_| true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, t: &str) -> bool {
        Pattern::new(p).unwrap_or_else(|e| panic!("{p}: {e}")).found(t)
    }

    #[test]
    fn it_matches_as_ecmascript_does() {
        assert!(m("^\\d{3}-\\d{4}$", "555-1234") && !m("^\\d{3}-\\d{4}$", "555-12345"));
        assert!(m("ab", "xxabyy"), "anywhere, unanchored");
        assert!(m("^(?:cat|dog)s?$", "dogs") && !m("^(?:cat|dog)s?$", "cow"));
        assert!(m("^[A-Za-z_][\\w-]*$", "a-b_c") && !m("^[A-Za-z_][\\w-]*$", "1ab"));
        assert!(m("^[^@\\s]+@[^@\\s]+$", "a@b") && !m("^[^@\\s]+@[^@\\s]+$", "a b@c"));
        assert!(m("^a{2,}$", "aaa") && !m("^a{2,}$", "a") && m("^a{1,2}b$", "aab") && !m("^a{1,2}b$", "aaab"));
        assert!(m("x{a", "x{a"), "a {{ that is no quantifier is literal");
        assert!(m("^(?!\\.)[a-z.]+$", "a.b") && !m("^(?!\\.)[a-z.]+$", ".ab"), "lookahead");
        assert!(m("^.*?b$", "aab") && m("^(a|ab)c$", "abc"), "backtracking");
        assert!(m("\\bcat\\b", "a cat!") && !m("\\bcat\\b", "concat"));
        assert!(m("^\\u00e9\\x41$", "éA") && m("^(?<y>\\d{4})$", "2026"));
        assert!(m("^(a*)*$", "aaaa") && !m("^(a*)*b$", "aaaa"), "an empty repeat ends");
        // Zod 4's email pattern.
        let zod =
            "^(?!\\.)(?!.*\\.\\.)([A-Za-z0-9_'+\\-\\.]*)[A-Za-z0-9_+-]@([A-Za-z0-9][A-Za-z0-9\\-]*\\.)+[A-Za-z]{2,}$";
        assert!(m(zod, "ada.l@x.io") && !m(zod, "ada..l@x.io") && !m(zod, "ada@x"));
        for bad in ["(?<=a)b", "(a)\\1", "[b-a]", "(a", "a)", "*a"] {
            assert!(Pattern::new(bad).is_err(), "{bad}");
        }
    }
}
