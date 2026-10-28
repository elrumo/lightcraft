//! Names a user's library folders skip: patterns like `*.fcpbundle` or `Proxy Media`, matched against one file or
//! folder name at a time (an admin sets them per folder, see [`crate::accounts::Folder::ignore`]).

/// Does `name` (one file or folder name, not a path) match one of the `ignore` patterns? `*` in a
/// pattern stands for any run of characters and `?` for one; case doesn't matter, blank patterns
/// match nothing. `*.fcpbundle` skips every Final Cut bundle, and with it everything inside.
pub fn is_ignored(ignore: &[String], name: &str) -> bool {
    if ignore.is_empty() {
        return false;
    }
    let name: Vec<char> = name.to_lowercase().chars().collect();
    ignore.iter().any(|p| {
        let p: Vec<char> = p.trim().to_lowercase().chars().collect();
        !p.is_empty() && glob(&p, &name)
    })
}

/// An ignore list as it is kept: trimmed, blanks and repeats dropped. A pattern names one file or
/// folder, so one with a `/` or `\` (a path, which would never match) is refused, as is a list
/// of more than 200 patterns or a pattern of more than 255 characters.
pub fn clean_ignore(list: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for p in list.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        if p.contains(['/', '\\']) {
            return Err(format!("`{p}`: a pattern matches one file or folder name (like `*.fcpbundle`), not a path"));
        }
        if p.chars().count() > 255 {
            return Err("a pattern is longer than 255 characters".into());
        }
        if !out.iter().any(|x| x.eq_ignore_ascii_case(p)) {
            out.push(p.to_string());
        }
    }
    if out.len() > 200 {
        return Err("at most 200 patterns".into());
    }
    Ok(out)
}

/// Wildcard match (`*`, `?`) by backtracking to the last `*`: linear in practice, no recursion.
fn glob(p: &[char], n: &[char]) -> bool {
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some(pi);
                mark = ni;
                pi += 1;
            }
            Some(&c) if c == '?' || n.get(ni) == Some(&c) => {
                pi += 1;
                ni += 1;
            }
            _ => match star {
                Some(s) => {
                    pi = s + 1;
                    mark += 1;
                    ni = mark;
                }
                None => return false,
            },
        }
    }
    p.get(pi..).is_some_and(|rest| rest.iter().all(|&c| c == '*'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn patterns_match_one_name_case_insensitively() {
        let ignore = list(&["*.fcpbundle", "Proxy Media", "IMG_?.jpg"]);
        assert!(is_ignored(&ignore, "Wedding.FCPBUNDLE"));
        assert!(is_ignored(&ignore, "proxy media"));
        assert!(is_ignored(&ignore, "img_1.jpg"));
        assert!(!is_ignored(&ignore, "img_12.jpg"));
        assert!(!is_ignored(&ignore, "Wedding.jpg"));
        assert!(!is_ignored(&[], "anything"));
        assert!(!is_ignored(&list(&["  "]), "anything"));
    }

    #[test]
    fn a_star_in_the_middle_backtracks() {
        assert!(is_ignored(&list(&["a*b*c"]), "aXXbYYbZc"));
        assert!(!is_ignored(&list(&["a*b*c"]), "aXXbYYbZ"));
        assert!(is_ignored(&list(&["*"]), ""));
    }

    #[test]
    fn a_list_is_cleaned_and_bounded() {
        assert_eq!(clean_ignore(&list(&[" a ", "A", "", "b"])).unwrap(), list(&["a", "b"]));
        assert!(clean_ignore(&list(&["dir/file"])).is_err());
        assert!(clean_ignore(&list(&["dir\\file"])).is_err());
        assert!(clean_ignore(&["x".repeat(256)]).is_err());
        let many: Vec<String> = (0..201).map(|i| format!("p{i}")).collect();
        assert!(clean_ignore(&many).is_err());
    }
}
