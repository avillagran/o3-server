use super::{Error, Result, Sink, MAX_OUTPUT};
use quick_xml::{events::Event, Reader};
use std::collections::HashMap;
#[derive(Default)]
pub(super) struct Node {
    pub name: String,
    pub attrs: HashMap<String, String>,
    pub children: Vec<Child>,
    space: Option<u64>,
    has_text: bool,
    choice: bool,
}
pub(super) enum Child {
    Node(Node),
    Text(String),
}
impl Node {
    pub fn attr(&self, key: &str) -> &str {
        self.attrs.get(key).map(String::as_str).unwrap_or("")
    }
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.children.iter().filter_map(|c| match c {
            Child::Node(n) => Some(n),
            _ => None,
        })
    }
    // Like nightly's all(), stop recursing once a matching descendant is found.
    pub fn all<'a>(&'a self, names: &[&str]) -> Vec<&'a Node> {
        let mut out = Vec::new();
        for n in self.nodes() {
            if names.contains(&n.name.as_str()) {
                out.push(n)
            } else {
                out.extend(n.all(names))
            }
        }
        out
    }
    pub fn first(&self, name: &str) -> Option<&Node> {
        self.all(&[name]).into_iter().next()
    }
    pub fn raw(&self) -> &str {
        self.children
            .iter()
            .find_map(|c| match c {
                Child::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .unwrap_or("")
    }
}
fn resolve_html(name: &str) -> Option<&'static str> {
    super::html_entities::multi(name).or_else(|| quick_xml::escape::resolve_html5_entity(name))
}
fn entity(value: &str, html: bool) -> Result<String> {
    quick_xml::escape::unescape_with(
        value,
        if html {
            resolve_html
        } else {
            quick_xml::escape::resolve_xml_entity
        },
    )
    .map(|s| s.into_owned())
    .map_err(|_| Error::Invalid)
}
pub(super) fn repeat(value: &str) -> Result<u64> {
    if value.is_empty() {
        return Ok(1);
    }
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid);
    }
    let n = value.parse::<u64>().unwrap_or(u64::MAX);
    if n == 0 {
        Err(Error::Invalid)
    } else {
        Ok(n)
    }
}
pub(super) fn parse(source: &str, odf: bool, html: bool) -> Result<Node> {
    let mut reader = Reader::from_str(source);
    let mut stack = vec![Node::default()];
    let mut count = 0;
    let mut roots = 0;
    loop {
        match reader.read_event().map_err(|_| Error::Invalid)? {
            event @ (Event::Start(_) | Event::Empty(_)) => {
                let empty = matches!(event, Event::Empty(_));
                let e = match event {
                    Event::Start(e) | Event::Empty(e) => e,
                    _ => unreachable!(),
                };
                count += 1;
                if count > 200_000 || stack.len() > 128 {
                    return Err(Error::Truncated);
                }
                let full = e.name().as_ref().to_owned();
                let mut n = Node {
                    name: full.rsplit(':').next().unwrap().into(),
                    ..Node::default()
                };
                for a in e.attributes() {
                    let a = a.map_err(|_| Error::Invalid)?;
                    let key = a.key.as_ref();
                    let v = entity(&a.value, html)?;
                    n.attrs.insert(key.into(), v.clone());
                    n.attrs.insert(key.rsplit(':').next().unwrap().into(), v);
                }
                if odf && n.name == "s" {
                    n.space = Some(repeat(n.attr("c"))?)
                }
                if stack.len() == 1 {
                    roots += 1;
                    if roots > 1 {
                        return Err(Error::Invalid);
                    }
                }
                // Empty events do not leave an open element on the stack.
                if empty {
                    stack.last_mut().unwrap().children.push(Child::Node(n))
                } else {
                    stack.push(n)
                }
            }
            Event::End(_) => {
                if stack.len() == 1 {
                    return Err(Error::Invalid);
                }
                let n = stack.pop().unwrap();
                let p = stack.last_mut().unwrap();
                if n.name == "Fallback" && p.choice {
                    continue;
                }
                if n.has_text {
                    p.has_text = true;
                    if n.name == "Choice" {
                        p.choice = true
                    }
                }
                p.children.push(Child::Node(n));
            }
            Event::Text(e) => {
                let raw = e.xml10_content();
                let t = entity(&raw, html)?;
                if stack.len() == 1 && !t.trim().is_empty() {
                    return Err(Error::Invalid);
                }
                let n = stack.last_mut().unwrap();
                if !t.trim().is_empty() {
                    n.has_text = true
                }
                n.children.push(Child::Text(t));
            }
            Event::CData(e) => {
                let t = e.xml10_content().into_owned();
                let n = stack.last_mut().unwrap();
                n.has_text |= !t.trim().is_empty();
                n.children.push(Child::Text(t));
            }
            Event::GeneralRef(e) => {
                let raw = e.xml10_content();
                let t = entity(&format!("&{raw};"), html)?;
                let n = stack.last_mut().unwrap();
                n.has_text |= !t.trim().is_empty();
                n.children.push(Child::Text(t));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1 || roots != 1 {
        return Err(Error::Invalid);
    }
    Ok(stack.pop().unwrap())
}
fn hidden(n: &Node) -> bool {
    matches!(
        n.name.as_str(),
        "script"
            | "style"
            | "binary-data"
            | "rPh"
            | "delText"
            | "moveFrom"
            | "instrText"
            | "delInstrText"
            | "tracked-changes"
            | "pPr"
    )
}
pub(super) fn text(n: &Node, out: &mut Sink) -> Result<()> {
    if hidden(n) {
        return Ok(());
    }
    match n.name.as_str() {
        "tab" | "cell" => return out.add("\t"),
        "br" | "line-break" | "cr" => return out.add("\n"),
        _ => {}
    }
    if let Some(count) = n.space {
        out.add(&" ".repeat(count.min(MAX_OUTPUT as u64) as usize))?;
        if count > MAX_OUTPUT as u64 {
            return Err(Error::Truncated);
        }
        return Ok(());
    }
    for c in &n.children {
        match c {
            Child::Text(t) => out.add(t)?,
            Child::Node(n) => text(n, out)?,
        }
    }
    Ok(())
}
pub(super) fn paragraphs(n: &Node, out: &mut Sink) -> Result<()> {
    if hidden(n) {
        return Ok(());
    }
    for c in &n.children {
        match c {
            Child::Text(t) => out.add(t)?,
            Child::Node(c) => match c.name.as_str() {
                "br" | "line-break" | "cr" => out.add("\n")?,
                "tbl" | "table" => table(c, out)?,
                "p" | "h" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let style = c.first("pStyle").map(|n| n.attr("val")).unwrap_or("");
                    let level = if c.name == "h" {
                        c.attr("outline-level").parse::<usize>().unwrap_or(1).max(1)
                    } else if c.name.starts_with('h') && c.name.len() == 2 {
                        c.name[1..].parse().unwrap_or(0)
                    } else {
                        style
                            .to_ascii_lowercase()
                            .strip_prefix("heading")
                            .and_then(|n| n.parse::<usize>().ok())
                            .filter(|n| (1..=6).contains(n))
                            .unwrap_or(0)
                    };
                    if level > 0 {
                        out.add(&format!("{} ", "#".repeat(level.min(6))))?
                    }
                    text(c, out)?;
                    out.add("\n")?;
                }
                _ => {
                    paragraphs(c, out)?;
                    if matches!(c.name.as_str(), "li" | "div" | "section") {
                        out.add("\n")?
                    }
                }
            },
        }
    }
    Ok(())
}
fn odf_value(n: &Node) -> String {
    let value = n.attr("value");
    match n.attr("value-type") {
        "void" => String::new(),
        "string" => n.attr("string-value").into(),
        "boolean" => n.attr("boolean-value").into(),
        "date" => n.attr("date-value").into(),
        "time" => n.attr("time-value").into(),
        "currency" if !value.trim().is_empty() => {
            format!("{value} {}", n.attr("currency")).trim().into()
        }
        "percentage" if !value.trim().is_empty() => format!("{value} [ODF value-type=percentage]"),
        _ => value.into(),
    }
}
pub(super) fn populated(n: &Node) -> bool {
    if !odf_value(n).trim().is_empty() {
        return true;
    }
    if hidden(n) || n.space.is_some() {
        return false;
    }
    n.children.iter().any(|c| match c {
        Child::Text(t) => !t.trim().is_empty(),
        Child::Node(n) => populated(n),
    })
}
pub(super) fn table(n: &Node, out: &mut Sink) -> Result<()> {
    let mut rows = 0;
    for row in n.all(&["tr", "table-row"]) {
        let mut cells = row
            .nodes()
            .filter(|c| {
                matches!(
                    c.name.as_str(),
                    "tc" | "table-cell" | "covered-table-cell" | "td" | "th"
                )
            })
            .collect::<Vec<_>>();
        while cells.last().is_some_and(|c| !populated(c)) {
            cells.pop();
        }
        if cells.is_empty() {
            continue;
        }
        for _ in 0..repeat(row.attr("number-rows-repeated"))? {
            rows += 1;
            if rows > 10_000 {
                return Err(Error::Truncated);
            }
            let mut column = 0;
            for cell in &cells {
                for _ in 0..repeat(cell.attr("number-columns-repeated"))? {
                    if column >= 512 {
                        return Err(Error::Truncated);
                    }
                    if column > 0 {
                        out.add("\t")?
                    }
                    column += 1;
                    out.cell_start();
                    paragraphs(cell, out)?;
                    out.fallback(&odf_value(cell))?;
                    out.cell_end();
                }
            }
            out.add("\n")?;
        }
    }
    Ok(())
}
