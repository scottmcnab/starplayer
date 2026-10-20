//! Read-only, one-off Hornet audit: cargo run --release -p starplayer-archive
//! --example legacy_corpus_check -- <index.csv> <mirror-root>
use std::{env, fs, io::Cursor, path::Path};
use sha2::{Digest, Sha256};
use starplayer_archive::{extract, list_modules};
use zip::ZipArchive;

// Handles quoted commas, doubled quotes and quoted newlines; rejects broken CSV.
fn parse_csv(input: &str) -> Result<Vec<Vec<String>>, &'static str> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' if quoted && characters.peek() == Some(&'"') => { field.push('"'); characters.next(); }
            '"' => quoted = !quoted,
            ',' if !quoted => row.push(std::mem::take(&mut field)),
            '\n' if !quoted => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            '\r' if !quoted && characters.peek() == Some(&'\n') => {}
            other => field.push(other),
        }
    }
    if quoted { return Err("unterminated CSV quote"); }
    if !field.is_empty() || !row.is_empty() { row.push(field); rows.push(row); }
    Ok(rows)
}

fn member_index(bytes: &[u8], name: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    for index in 0..archive.len() {
        if archive.by_index_raw(index)?.name()? == name { return Ok(index); }
    }
    Err(format!("member not found: {name}").into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = env::args().collect();
    if arguments.len() != 3 { return Err("usage: legacy_corpus_check <index.csv> <mirror-root>".into()); }
    let input = fs::read_to_string(&arguments[1])?;
    let rows = parse_csv(&input)?;
    let header = rows.first().ok_or("empty CSV")?;
    let column = |name: &str| header.iter().position(|value| value == name).ok_or_else(|| format!("missing column {name}"));
    let source_column = column("source_path")?;
    let container_column = column("container_member")?;
    let member_column = column("member_path")?;
    let method_column = column("compression_method")?;
    let hash_column = column("sha256")?;
    let extension_column = column("extension")?;
    let mut total = 0;
    let mut matched = 0;
    let mut mismatched = 0;
    let mut failed = 0;
    let mut module_total = 0;
    let mut module_matched = 0;
    let mut nested = 0;
    for row in rows.iter().skip(1) {
        if row.len() != header.len() { return Err(format!("CSV row has {} fields, expected {}", row.len(), header.len()).into()); }
        let method = &row[method_column];
        if method != "implode" && method != "shrink" && !method.starts_with("reduce") { continue; }
        total += 1;
        let is_module = matches!(row[extension_column].to_ascii_lowercase().as_str(), "mod" | "s3m" | "mtm" | "xm" | "it");
        if is_module { module_total += 1; }
        let label = format!("{} / {} / {}", row[source_column], row[container_column], row[member_column]);
        let result = (|| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            let mut bytes = fs::read(Path::new(&arguments[2]).join(&row[source_column]))?;
            if !row[container_column].is_empty() {
                nested += 1;
                bytes = extract(&bytes, member_index(&bytes, &row[container_column])?)?;
            }
            let index = member_index(&bytes, &row[member_column])?;
            if is_module && !list_modules(&bytes)?.iter().any(|entry| entry.index == index) { return Err("module missing from list_modules".into()); }
            Ok(extract(&bytes, index)?)
        })();
        match result {
            Ok(bytes) => {
                let actual = format!("{:x}", Sha256::digest(&bytes));
                if actual == row[hash_column] {
                    matched += 1;
                    if is_module { module_matched += 1; }
                } else {
                    mismatched += 1;
                    println!("MISMATCH {label}: expected {}, got {actual}", row[hash_column]);
                }
            }
            Err(error) => { failed += 1; println!("FAILED {label}: {error}"); }
        }
    }
    println!("total={total} matched={matched} mismatched={mismatched} failed={failed} nested={nested}");
    println!("modules: total={module_total} matched={module_matched}");
    if mismatched + failed != 0 { return Err("legacy corpus check failed".into()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_csv;

    #[test]
    fn quoted_fields_and_newlines() {
        assert_eq!(parse_csv("a,\"b,c\",\"d\"\"e\"\r\n\"f\ng\",,z\n").unwrap(), vec![vec!["a", "b,c", "d\"e"], vec!["f\ng", "", "z"]]);
        assert!(parse_csv("\"unfinished").is_err());
    }
}
