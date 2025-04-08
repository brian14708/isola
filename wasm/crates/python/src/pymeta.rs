use std::sync::LazyLock;

use memchr::memmem::Finder;

static FINDER: LazyLock<Finder> = LazyLock::new(|| Finder::new(b"# /// script"));

pub fn parse_pep723(contents: &[u8]) -> Option<String> {
    let index = FINDER.find(contents)?;

    if !(index == 0 || matches!(contents[index - 1], b'\r' | b'\n')) {
        return None;
    }

    let contents = &contents[index..];
    let contents = std::str::from_utf8(contents).ok()?;
    let mut lines = contents.lines();

    if lines.next().is_none_or(|line| line != "# /// script") {
        return None;
    }

    let mut toml = vec![];
    for line in lines {
        let Some(line) = line.strip_prefix('#') else {
            break;
        };

        if line.is_empty() {
            toml.push("");
            continue;
        }

        let Some(line) = line.strip_prefix(' ') else {
            break;
        };
        toml.push(line);
    }

    let index = toml.iter().rev().position(|line| *line == "///")?;
    let index = toml.len() - index;
    toml.truncate(index - 1);
    let metadata = toml.join("\n") + "\n";
    Some(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_space() {
        let contents = r"
# /// script
# requires-python = '>=3.11'
# dependencies = [
#   'requests<3',
#   'rich',
# ]
# ///
    ";

        assert_eq!(
            parse_pep723(contents.as_bytes()).unwrap(),
            "requires-python = '>=3.11'\ndependencies = [\n  'requests<3',\n  'rich',\n]\n"
        );
    }
}
