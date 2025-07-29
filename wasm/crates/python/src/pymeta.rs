pub fn parse_pep723(contents: &str) -> Option<String> {
    let index = contents.find("# /// script")?;

    if !(index == 0 || contents.as_bytes()[index - 1] == b'\n') {
        return None;
    }

    let contents = &contents[index..];
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
            parse_pep723(contents).unwrap(),
            "requires-python = '>=3.11'\ndependencies = [\n  'requests<3',\n  'rich',\n]\n"
        );
    }
}
