use super::{
    xml::{self, Node},
    Error, Result, Sink, MAX_INPUT,
};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
const MAX_EXPANDED: u64 = 32 * 1024 * 1024;
fn decode(data: &[u8]) -> String {
    let encoding = if data.starts_with(&[255, 254]) || data.starts_with(&[b'<', 0]) {
        encoding_rs::UTF_16LE
    } else if data.starts_with(&[254, 255]) || data.starts_with(&[0, b'<']) {
        encoding_rs::UTF_16BE
    } else {
        encoding_rs::UTF_8
    };
    encoding.decode(data).0.into_owned()
}
fn archive(path: &Path) -> Result<HashMap<String, String>> {
    let mut f = std::fs::File::open(path).map_err(|_| Error::Invalid)?;
    // Bound central-directory admission before the ZIP library allocates its index.
    let length = f.metadata().map_err(|_| Error::Invalid)?.len();
    f.seek(SeekFrom::Start(length.saturating_sub(65_557)))
        .map_err(|_| Error::Invalid)?;
    let mut tail = Vec::new();
    f.read_to_end(&mut tail).map_err(|_| Error::Invalid)?;
    if let Some(i) = tail.windows(4).rposition(|p| p == b"PK\x05\x06") {
        if let Some(header) = tail.get(i..i + 22) {
            let count = u16::from_le_bytes([header[10], header[11]]);
            if count > 2048 {
                return Err(Error::Truncated);
            }
        }
    }
    f.seek(SeekFrom::Start(0)).map_err(|_| Error::Invalid)?;
    let mut zip = zip::ZipArchive::new(f).map_err(|_| Error::Invalid)?;
    if zip.len() > 2048 {
        return Err(Error::Truncated);
    }
    let mut files = HashMap::new();
    let mut expanded = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index_data(i).map_err(|_| Error::Invalid)?;
        if entry.encrypted() {
            return Err(Error::Invalid);
        }
        let name = entry.name().map_err(|_| Error::Invalid)?.into_owned();
        // Never materialize archive paths on disk. References can only address this map.
        if name.contains('\\') || name.starts_with('/') || name.split('/').any(|s| s == "..") {
            return Err(Error::Invalid);
        }
        if !matches!(
            name.rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "xml" | "rels" | "xhtml" | "html" | "htm" | "opf" | "svg"
        ) {
            continue;
        }
        let size = entry.size();
        expanded = expanded.saturating_add(size);
        if size > MAX_EXPANDED
            || size > 1024.max(entry.compressed_size().saturating_mul(200))
            || expanded > MAX_EXPANDED
        {
            return Err(Error::Truncated);
        }
        let mut data = Vec::new();
        let mut entry = zip.by_index(i).map_err(|_| Error::Invalid)?;
        entry
            .by_ref()
            .take(size + 1)
            .read_to_end(&mut data)
            .map_err(|_| Error::Invalid)?;
        if data.len() as u64 != size {
            return Err(Error::Invalid);
        }
        files.insert(name, decode(&data));
    }
    Ok(files)
}
struct Parts {
    files: HashMap<String, String>,
    notes: Vec<String>,
    odf: bool,
    html: bool,
}
impl Parts {
    fn skip(&mut self, name: &str) {
        self.notes.push(format!(
            "\n[Skipped document part: {name} (missing or invalid XML).]\n"
        ))
    }
    fn read(&mut self, name: &str) -> Result<Node> {
        match self
            .files
            .get(name)
            .ok_or(Error::Invalid)
            .and_then(|s| xml::parse(s, self.odf, self.html))
        {
            Ok(n) => Ok(n),
            Err(Error::Truncated) => Err(Error::Truncated),
            Err(_) => {
                self.skip(name);
                Ok(Node::default())
            }
        }
    }
    fn relations(&mut self, name: &str) -> Result<HashMap<String, String>> {
        Ok(self
            .read(name)?
            .all(&["Relationship"])
            .into_iter()
            .filter(|n| n.attr("TargetMode") != "External")
            .map(|n| (n.attr("Id").into(), n.attr("Target").into()))
            .collect())
    }
}
fn resolve(base: &str, target: &str) -> String {
    let path = if target.starts_with('/') {
        target.to_owned()
    } else {
        format!("{base}/{target}")
    };
    let mut parts = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    parts.join("/")
}
pub(super) fn extract(path: &Path, extension: &str, out: &mut Sink) -> Result<()> {
    if std::fs::metadata(path).map_err(|_| Error::Invalid)?.len() > MAX_INPUT {
        return Err(Error::Truncated);
    }
    let mut p = Parts {
        files: archive(path)?,
        notes: vec![],
        odf: matches!(extension, "odt" | "ods" | "odp"),
        html: extension == "epub",
    };
    match extension {
        "docx" => xml::paragraphs(&p.read("word/document.xml")?, out)?,
        "odt" | "ods" | "odp" => {
            let content = p.read("content.xml")?;
            if extension == "odt" {
                xml::paragraphs(&content, out)?
            } else {
                for (index, n) in content
                    .all(&[if extension == "ods" { "table" } else { "page" }])
                    .into_iter()
                    .enumerate()
                {
                    if extension == "ods" {
                        out.add(&format!(
                            "\n\n## Sheet: {}\n",
                            if n.attrs.contains_key("name") {
                                n.attr("name")
                            } else {
                                "Untitled"
                            }
                        ))?;
                        xml::table(n, out)?
                    } else {
                        out.add(&format!("\n\n## Slide {}\n", index + 1))?;
                        xml::paragraphs(n, out)?
                    }
                }
            }
        }
        "xlsx" => xlsx(&mut p, out)?,
        "pptx" => {
            let rels = p.relations("ppt/_rels/presentation.xml.rels")?;
            let slides = p.read("ppt/presentation.xml")?;
            for (index, slide) in slides.all(&["sldId"]).into_iter().enumerate() {
                out.add(&format!("\n\n## Slide {}\n", index + 1))?;
                let Some(target) = rels
                    .get(slide.attr("r:id"))
                    .or_else(|| rels.get(slide.attr("id")))
                else {
                    p.skip(&format!("slide relationship {}", index + 1));
                    continue;
                };
                let name = resolve("ppt", target);
                xml::paragraphs(&p.read(&name)?, out)?;
                let (dir, file) = name.rsplit_once('/').unwrap_or(("", &name));
                let slide_rels = format!("{dir}/_rels/{file}.rels");
                if !p.files.contains_key(&slide_rels) {
                    continue;
                }
                let rels = p.read(&slide_rels)?;
                for r in rels.all(&["Relationship"]) {
                    if r.attr("Type").ends_with("/notesSlide")
                        && r.attr("TargetMode") != "External"
                        && !r.attr("Target").is_empty()
                    {
                        out.add("Notes:\n")?;
                        xml::paragraphs(&p.read(&resolve(dir, r.attr("Target")))?, out)?
                    }
                }
            }
        }
        "epub" => {
            let container = p.read("META-INF/container.xml")?;
            let root = container
                .first("rootfile")
                .map(|n| n.attr("full-path"))
                .unwrap_or("");
            if root.is_empty() {
                if p.notes.is_empty() {
                    p.skip("EPUB package")
                }
            } else {
                let book = p.read(root)?;
                let items = book
                    .all(&["item"])
                    .into_iter()
                    .map(|n| (n.attr("id"), n.attr("href")))
                    .collect::<HashMap<_, _>>();
                for (index, item) in book.all(&["itemref"]).into_iter().enumerate() {
                    out.add(&format!("\n\n## Chapter {}\n", index + 1))?;
                    let Some(href) = items.get(item.attr("idref")) else {
                        p.skip(&format!("chapter {}", index + 1));
                        continue;
                    };
                    let Some(decoded) = percent_decode(href.split('#').next().unwrap_or("")) else {
                        p.skip(&format!("chapter {}", index + 1));
                        continue;
                    };
                    xml::paragraphs(
                        &p.read(&resolve(
                            root.rsplit_once('/').map_or("", |p| p.0),
                            &decoded,
                        ))?,
                        out,
                    )?;
                }
            }
        }
        _ => return Err(Error::Invalid),
    }
    for note in p.notes {
        out.add(&note)?
    }
    Ok(())
}
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(
                u8::from_str_radix(std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?, 16).ok()?,
            );
            i += 3
        } else {
            out.push(bytes[i]);
            i += 1
        }
    }
    String::from_utf8(out).ok()
}
fn xlsx(p: &mut Parts, out: &mut Sink) -> Result<()> {
    let workbook = p.read("xl/workbook.xml")?;
    let date = if workbook
        .first("workbookPr")
        .is_some_and(|n| matches!(n.attr("date1904"), "true" | "1"))
    {
        "1904"
    } else {
        "1900"
    };
    let styles = if p.files.contains_key("xl/styles.xml") {
        p.read("xl/styles.xml")?
    } else {
        Node::default()
    };
    let customs = styles
        .all(&["numFmt"])
        .into_iter()
        .map(|n| (n.attr("numFmtId"), n.attr("formatCode")))
        .collect::<HashMap<_, _>>();
    let formats = styles
        .first("cellXfs")
        .map(|n| n.nodes().collect::<Vec<_>>())
        .unwrap_or_default();
    let strings = if p.files.contains_key("xl/sharedStrings.xml") {
        p.read("xl/sharedStrings.xml")?
    } else {
        Node::default()
    };
    let strings = strings.all(&["si"]);
    let rels = p.relations("xl/_rels/workbook.xml.rels")?;
    let populated = |c: &Node| {
        let v = c.first("v");
        match c.attr("t") {
            "s" => v
                .and_then(|n| n.raw().trim().parse::<usize>().ok())
                .and_then(|i| strings.get(i))
                .is_some_and(|n| xml::populated(n)),
            "inlineStr" => xml::populated(c),
            _ => v.is_some_and(xml::populated) || c.all(&["f"]).into_iter().any(xml::populated),
        }
    };
    for sheet in workbook.all(&["sheet"]) {
        out.add(&format!("\n## Sheet: {}\n", sheet.attr("name")))?;
        let Some(target) = rels
            .get(sheet.attr("r:id"))
            .or_else(|| rels.get(sheet.attr("id")))
        else {
            p.skip(&format!("sheet relationship {}", sheet.attr("name")));
            continue;
        };
        let data = p.read(&resolve("xl", target))?;
        let mut row_count = 0;
        for row in data.all(&["row"]) {
            let mut cells = Vec::<Option<&Node>>::new();
            let mut next = 0;
            let mut truncated = false;
            for c in row.nodes().filter(|n| n.name == "c") {
                let letters = c
                    .attr("r")
                    .bytes()
                    .take_while(u8::is_ascii_uppercase)
                    .collect::<Vec<_>>();
                let col = if letters.is_empty() {
                    next
                } else {
                    letters
                        .iter()
                        .fold(0usize, |n, b| {
                            n.saturating_mul(26).saturating_add((b - 64) as usize)
                        })
                        .saturating_sub(1)
                };
                next = col.saturating_add(1);
                if !populated(c) {
                    continue;
                }
                if col >= 512 {
                    truncated = true;
                    continue;
                }
                if cells.len() <= col {
                    cells.resize(col + 1, None)
                }
                cells[col] = Some(c);
            }
            if cells.is_empty() && !truncated {
                continue;
            }
            row_count += 1;
            if row_count > 10_000 {
                return Err(Error::Truncated);
            }
            for (col, c) in cells.iter().enumerate() {
                if col > 0 {
                    out.add("\t")?
                }
                let Some(c) = c else { continue };
                let value = c.first("v");
                let raw = value.map(|v| v.raw()).unwrap_or("");
                out.cell_start();
                match c.attr("t") {
                    "s" => {
                        if let Some(shared) = raw
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| strings.get(i))
                        {
                            xml::text(shared, out)?
                        }
                    }
                    "inlineStr" => xml::text(c, out)?,
                    _ => {
                        if let Some(v) = value.filter(|v| xml::populated(v)) {
                            xml::text(v, out)?
                        } else {
                            for f in c.all(&["f"]) {
                                xml::text(f, out)?
                            }
                        }
                    }
                }
                out.cell_end();
                if matches!(c.attr("t"), "" | "n") && !raw.trim().is_empty() {
                    let fmt = c.attr("s").parse::<usize>().unwrap_or(0);
                    let id = formats.get(fmt).map(|n| n.attr("numFmtId")).unwrap_or("");
                    if !id.is_empty() && id != "0" {
                        out.add(&format!(
                            " [Excel numFmtId={id}{}; date system {date}]",
                            customs
                                .get(id)
                                .map(|s| format!("; format={s}"))
                                .unwrap_or_default()
                        ))?
                    }
                }
            }
            out.add("\n")?;
            if truncated {
                return Err(Error::Truncated);
            }
        }
    }
    Ok(())
}
