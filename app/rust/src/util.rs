//! 文本工具:自然排序比较。

use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

/// 自然排序比较:连续数字段按数值比较,其余按字符(忽略大小写)比较。
/// 例如 `page2 < page10`,契合漫画页命名习惯。
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ca = a.chars().peekable();
    let mut cb = b.chars().peekable();
    loop {
        match (ca.peek().copied(), cb.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    let na = take_number(&mut ca);
                    let nb = take_number(&mut cb);
                    match na.cmp(&nb) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                } else {
                    let lx = x.to_lowercase().next().unwrap_or(x);
                    let ly = y.to_lowercase().next().unwrap_or(y);
                    match lx.cmp(&ly) {
                        Ordering::Equal => {
                            ca.next();
                            cb.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

/// 从迭代器取出完整数字段,返回其数值。
fn take_number(it: &mut Peekable<Chars>) -> u64 {
    let mut n: u64 = 0;
    while let Some(&c) = it.peek() {
        if let Some(d) = c.to_digit(10) {
            n = n.saturating_mul(10).saturating_add(d as u64);
            it.next();
        } else {
            break;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;
    use std::cmp::Ordering;

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("page1", "page2"), Ordering::Less);
        assert_eq!(natural_cmp("page2", "page10"), Ordering::Less);
        assert_eq!(natural_cmp("page10", "page2"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        let mut v = vec!["p10", "p1", "p2", "P3"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["p1", "p2", "P3", "p10"]);
    }
}
