use anyhow::Result;
use serde_json::Value;

pub fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Print an array of objects with the selected columns. `key` supports dotted
/// paths such as `user.id`.
pub fn print_rows(value: &Value, columns: &[(&str, &str)]) {
    let rows: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    if rows.is_empty() {
        println!("(none)");
        return;
    }
    let headers: Vec<&str> = columns.iter().map(|(_, header)| *header).collect();
    let body: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            columns
                .iter()
                .map(|(key, _)| render(lookup(row, key)))
                .collect()
        })
        .collect();
    print_table(&headers, &body);
}

/// Print selected fields of a single object.
pub fn print_fields(value: &Value, fields: &[(&str, &str)]) {
    let width = fields
        .iter()
        .map(|(_, label)| label.len())
        .max()
        .unwrap_or(0);
    for (key, label) in fields {
        println!("{label:<width$}  {}", render(lookup(value, key)));
    }
}

fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let header_line = headers
        .iter()
        .enumerate()
        .map(|(index, header)| pad(header, widths[index]))
        .collect::<Vec<_>>()
        .join("  ");
    println!("{}", header_line.trim_end());
    let rule = widths
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>()
        .join("  ");
    println!("{}", rule.trim_end());
    for row in rows {
        let line = row
            .iter()
            .enumerate()
            .map(|(index, cell)| pad(cell, widths[index]))
            .collect::<Vec<_>>()
            .join("  ");
        println!("{}", line.trim_end());
    }
}

fn pad(value: &str, width: usize) -> String {
    let length = value.chars().count();
    if length >= width {
        value.to_string()
    } else {
        format!("{value}{}", " ".repeat(width - length))
    }
}

fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn render(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "-".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| render(Some(item)))
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::Object(_)) => "<object>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_lookup_and_rendering() {
        let value = serde_json::json!({
            "user": {"id": "u1", "name": "Ada"},
            "roles": ["admin", "member"],
            "missing": null
        });
        assert_eq!(render(lookup(&value, "user.id")), "u1");
        assert_eq!(render(lookup(&value, "roles")), "admin, member");
        assert_eq!(render(lookup(&value, "missing")), "-");
        assert_eq!(render(lookup(&value, "user.absent")), "-");
    }
}
