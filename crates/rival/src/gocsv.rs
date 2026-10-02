//! pflag's `readAsCSV`: Go `encoding/csv` `Reader.Read` for the first record
//! of a `StringSlice` flag value (comma `,`, strict quotes, no comments).

/// Go `readAsCSV(val)`. An empty value is an empty list. Errors carry Go's
/// `*csv.ParseError` text (or `EOF` for a value of only line breaks).
pub fn read_as_csv(val: &str) -> Result<Vec<String>, String> {
    if val.is_empty() {
        return Ok(Vec::new());
    }
    let mut reader = Lines {
        rest: val.as_bytes(),
        num_line: 0,
    };

    // Skip empty lines.
    let mut line;
    loop {
        match reader.read_line() {
            None => return Err("EOF".to_string()),
            Some(l) if l == b"\n" => continue,
            Some(l) => {
                line = l;
                break;
            }
        }
    }

    let rec_line = reader.num_line;
    let mut col = 1usize;
    let mut buf: Vec<u8> = Vec::new();
    let mut ends: Vec<usize> = Vec::new();
    let parse_error = |line: usize, col: usize, what: &str| {
        let what = match what {
            "bare" => "bare \" in non-quoted-field",
            _ => "extraneous or missing \" in quoted-field",
        };
        if rec_line != line {
            format!("record on line {rec_line}; parse error on line {line}, column {col}: {what}")
        } else {
            format!("parse error on line {line}, column {col}: {what}")
        }
    };

    'field: loop {
        if line.first() != Some(&b'"') {
            // Non-quoted field.
            let comma = line.iter().position(|&b| b == b',');
            let field = match comma {
                Some(i) => &line[..i],
                None => &line[..line.len() - nl_len(&line)],
            };
            if let Some(j) = field.iter().position(|&b| b == b'"') {
                return Err(parse_error(reader.num_line, col + j, "bare"));
            }
            buf.extend_from_slice(field);
            ends.push(buf.len());
            match comma {
                Some(i) => {
                    line = line[i + 1..].to_vec();
                    col += i + 1;
                    continue 'field;
                }
                None => break 'field,
            }
        }
        // Quoted field.
        line = line[1..].to_vec();
        col += 1;
        loop {
            if let Some(i) = line.iter().position(|&b| b == b'"') {
                buf.extend_from_slice(&line[..i]);
                line = line[i + 1..].to_vec();
                col += i + 1;
                match line.first() {
                    Some(b'"') => {
                        buf.push(b'"');
                        line = line[1..].to_vec();
                        col += 1;
                    }
                    Some(b',') => {
                        line = line[1..].to_vec();
                        col += 1;
                        ends.push(buf.len());
                        continue 'field;
                    }
                    _ if nl_len(&line) == line.len() => {
                        ends.push(buf.len());
                        break 'field;
                    }
                    _ => return Err(parse_error(reader.num_line, col - 1, "quote")),
                }
            } else if !line.is_empty() {
                // End of line inside quotes: the field continues on the next.
                buf.extend_from_slice(&line);
                col += line.len();
                match reader.read_line() {
                    Some(next) => {
                        line = next;
                        col = 1;
                    }
                    None => line = Vec::new(),
                }
            } else {
                // Abrupt end of input inside quotes.
                return Err(parse_error(reader.line_of_eof(), col, "quote"));
            }
        }
    }

    let text = String::from_utf8_lossy(&buf).into_owned();
    let mut out = Vec::with_capacity(ends.len());
    let mut start = 0;
    for end in ends {
        out.push(text.get(start..end).unwrap_or_default().to_string());
        start = end;
    }
    Ok(out)
}

/// Go's `\n` length helper.
fn nl_len(b: &[u8]) -> usize {
    usize::from(b.last() == Some(&b'\n'))
}

/// Go `Reader.readLine` over an in-memory value.
struct Lines<'a> {
    rest: &'a [u8],
    num_line: usize,
}

impl Lines<'_> {
    /// The next line including its `\n`, with `\r\n` normalized to `\n` and
    /// a final `\r` before EOF dropped. `None` at EOF.
    fn read_line(&mut self) -> Option<Vec<u8>> {
        if self.rest.is_empty() {
            return None;
        }
        let end = self
            .rest
            .iter()
            .position(|&b| b == b'\n')
            .map_or(self.rest.len(), |i| i + 1);
        let mut line = self.rest[..end].to_vec();
        self.rest = &self.rest[end..];
        if line.last() != Some(&b'\n') && line.last() == Some(&b'\r') {
            line.pop();
        }
        self.num_line += 1;
        let n = line.len();
        if n >= 2 && line[n - 2] == b'\r' && line[n - 1] == b'\n' {
            line.truncate(n - 1);
            line[n - 2] = b'\n';
        }
        Some(line)
    }

    /// Go reports an unterminated quote at the last line it read.
    fn line_of_eof(&self) -> usize {
        self.num_line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(v: &str) -> Vec<String> {
        read_as_csv(v).unwrap()
    }

    #[test]
    fn splits_like_pflag_string_slices() {
        assert_eq!(ok(""), Vec::<String>::new());
        assert_eq!(ok("codex"), ["codex"]);
        assert_eq!(ok("codex,claude"), ["codex", "claude"]);
        assert_eq!(ok(" codex , claude"), [" codex ", " claude"]);
        assert_eq!(ok("a,,b,"), ["a", "", "b", ""]);
        assert_eq!(ok("\"a,b\",c"), ["a,b", "c"]);
        assert_eq!(ok("\"say \"\"hi\"\"\""), ["say \"hi\""]);
        // Only the first record counts; empty leading lines are skipped.
        assert_eq!(ok("a,b\nc"), ["a", "b"]);
        assert_eq!(ok("\n\nx"), ["x"]);
        assert_eq!(ok("a\r\n"), ["a"]);
        assert_eq!(ok("a\r"), ["a"]);
        // A quoted field may span lines.
        assert_eq!(ok("\"a\nb\",c"), ["a\nb", "c"]);
    }

    #[test]
    fn errors_match_go_parse_errors() {
        assert_eq!(read_as_csv("\n").unwrap_err(), "EOF");
        assert_eq!(
            read_as_csv("a\"b").unwrap_err(),
            "parse error on line 1, column 2: bare \" in non-quoted-field"
        );
        assert_eq!(
            read_as_csv("x,a\"b").unwrap_err(),
            "parse error on line 1, column 4: bare \" in non-quoted-field"
        );
        assert_eq!(
            read_as_csv("\"abc").unwrap_err(),
            "parse error on line 1, column 5: extraneous or missing \" in quoted-field"
        );
        assert_eq!(
            read_as_csv("\"a\"b").unwrap_err(),
            "parse error on line 1, column 3: extraneous or missing \" in quoted-field"
        );
        assert_eq!(
            read_as_csv("\"a\nb\"c").unwrap_err(),
            "record on line 1; parse error on line 2, column 2: extraneous or missing \" in quoted-field"
        );
    }
}
