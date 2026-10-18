//! Op words, as typed on the command line: `set-attr 0 fill red`.

use xmesh::Op;

/// Parses a path: `/` is the root element, `0/2` the third child of its first child.
pub fn parse_path(word: &str) -> Result<Vec<u32>, String> {
    if word == "/" {
        return Ok(vec![]);
    }
    word.trim_matches('/')
        .split('/')
        .map(|p| p.parse::<u32>().map_err(|_| format!("bad path {word:?}")))
        .collect()
}

fn number(word: &str, what: &str) -> Result<u32, String> {
    word.parse().map_err(|_| format!("bad {what} {word:?}"))
}

/// One op from its words. The last argument of `insert` (the XML) and
/// `insert-text` (the text) takes the rest of the words, joined by spaces.
pub fn parse_op_words(words: &[String]) -> Result<Op, String> {
    let arg = |i: usize| {
        words
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| format!("{}: missing arguments", words[0]))
    };
    let rest = |i: usize| -> Result<String, String> {
        if words.len() <= i {
            return Err(format!("{}: missing arguments", words[0]));
        }
        Ok(words[i..].join(" "))
    };
    let exact = |n: usize| -> Result<(), String> {
        if words.len() == n {
            Ok(())
        } else {
            Err(format!("{}: expected {} arguments", words[0], n - 1))
        }
    };
    let Some(verb) = words.first() else {
        return Err("no op given".into());
    };
    match verb.as_str() {
        "set-attr" => {
            exact(4)?;
            Ok(Op::SetAttr {
                path: parse_path(arg(1)?)?,
                name: arg(2)?.into(),
                value: arg(3)?.into(),
            })
        }
        "remove-attr" => {
            exact(3)?;
            Ok(Op::RemoveAttr {
                path: parse_path(arg(1)?)?,
                name: arg(2)?.into(),
            })
        }
        "insert" => Ok(Op::Insert {
            parent: parse_path(arg(1)?)?,
            index: number(arg(2)?, "index")?,
            xml: rest(3)?,
        }),
        "remove" => {
            exact(2)?;
            Ok(Op::Remove {
                path: parse_path(arg(1)?)?,
            })
        }
        "move" => {
            exact(4)?;
            Ok(Op::Move {
                path: parse_path(arg(1)?)?,
                new_parent: parse_path(arg(2)?)?,
                index: number(arg(3)?, "index")?,
            })
        }
        "insert-text" => Ok(Op::InsertText {
            path: parse_path(arg(1)?)?,
            offset: number(arg(2)?, "offset")?,
            text: rest(3)?,
        }),
        "delete-text" => {
            exact(4)?;
            Ok(Op::DeleteText {
                path: parse_path(arg(1)?)?,
                offset: number(arg(2)?, "offset")?,
                len: number(arg(3)?, "length")?,
            })
        }
        other => Err(format!("unknown op {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn each_word() {
        assert_eq!(
            parse_op_words(&w("set-attr 0/2 fill red")).unwrap(),
            Op::SetAttr {
                path: vec![0, 2],
                name: "fill".into(),
                value: "red".into()
            }
        );
        assert_eq!(
            parse_op_words(&w("remove-attr / inkscape:label")).unwrap(),
            Op::RemoveAttr {
                path: vec![],
                name: "inkscape:label".into()
            }
        );
        assert_eq!(
            parse_op_words(&w(r#"insert / 0 <circle r="5"/> <rect/>"#)).unwrap(),
            Op::Insert {
                parent: vec![],
                index: 0,
                xml: r#"<circle r="5"/> <rect/>"#.into()
            }
        );
        assert_eq!(
            parse_op_words(&w("remove 1/0")).unwrap(),
            Op::Remove { path: vec![1, 0] }
        );
        assert_eq!(
            parse_op_words(&w("move 2 1 0")).unwrap(),
            Op::Move {
                path: vec![2],
                new_parent: vec![1],
                index: 0
            }
        );
        assert_eq!(
            parse_op_words(&w("insert-text 1/0 3 hello world")).unwrap(),
            Op::InsertText {
                path: vec![1, 0],
                offset: 3,
                text: "hello world".into()
            }
        );
        assert_eq!(
            parse_op_words(&w("delete-text 1/0 3 2")).unwrap(),
            Op::DeleteText {
                path: vec![1, 0],
                offset: 3,
                len: 2
            }
        );
    }

    #[test]
    fn bad_input() {
        for bad in [
            "",
            "frobnicate 0",
            "set-attr 0 fill",
            "remove x/1",
            "move 0 1",
            "delete-text 0 a 1",
        ] {
            assert!(parse_op_words(&w(bad)).is_err(), "{bad:?}");
        }
    }
}
