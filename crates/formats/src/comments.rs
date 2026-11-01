//! Print Markup for the text formats: HTML, Markdown and plain text copies end with the
//! document's comments, numbered in document order, each with its author, the text it comments
//! on, what it says and its replies (like the notes at the end of a printed document).

use std::collections::{BTreeMap, HashMap, HashSet};

use wordcraft_doc::para::{InlineObject, OBJ};
use wordcraft_doc::{Document, StoryRef};

/// Longest quoted anchor text (characters).
const MAX_QUOTE: usize = 200;
const MAX_COMMENTS: usize = 5_000;
const MAX_REPLIES: usize = 200;
const MAX_PARENT_CHAIN: usize = 64;

/// One comment as written out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub author: String,
    /// The text it comments on (shortened), or empty.
    pub quote: String,
    pub text: String,
    /// (author, text) of each reply, oldest first.
    pub replies: Vec<(String, String)>,
}

fn root(doc: &Document, id: u32) -> u32 {
    let mut cur = id;
    for _ in 0..MAX_PARENT_CHAIN {
        match doc.comments.get(&cur).and_then(|c| c.parent) {
            Some(p) if p != cur && doc.comments.contains_key(&p) => cur = p,
            _ => break,
        }
    }
    cur
}

/// The author's name on one line (like the comment text and quote): a line break in it would
/// end the Markdown list or the plain-text entry the comment is written in.
fn author(a: &str) -> String {
    let a = a.split_whitespace().collect::<Vec<_>>().join(" ");
    if a.is_empty() { "Author".to_string() } else { a }
}

fn text_of(doc: &Document, part: u32) -> String {
    doc.plain_text(StoryRef::Part(part)).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The document's top-level comments: those anchored in the body in document order, then the
/// rest; replies go under the comment they answer.
pub fn notes(doc: &Document) -> Vec<Note> {
    if doc.comments.is_empty() {
        return Vec::new();
    }
    // Anchor order and the text between each comment's start and end.
    let mut order: Vec<u32> = Vec::new();
    let mut seen = HashSet::new();
    let mut quotes: HashMap<u32, String> = HashMap::new();
    let mut open: Vec<u32> = Vec::new();
    for path in doc.para_paths(StoryRef::Body) {
        let Some(p) = doc.para(StoryRef::Body, &path) else { continue };
        for (off, c) in p.text.char_indices() {
            if c == OBJ {
                match p.object_at(off) {
                    Some(InlineObject::CommentStart { id }) => {
                        if seen.insert(*id) {
                            order.push(*id);
                        }
                        if !open.contains(id) && open.len() < 64 {
                            open.push(*id);
                        }
                    }
                    Some(InlineObject::CommentEnd { id }) => open.retain(|x| x != id),
                    _ => {}
                }
                continue;
            }
            for id in &open {
                let q = quotes.entry(*id).or_default();
                if q.chars().count() <= MAX_QUOTE {
                    q.push(c);
                }
            }
        }
        for id in &open {
            quotes.entry(*id).or_default().push(' ');
        }
    }
    for id in doc.comments.keys() {
        if seen.insert(*id) {
            order.push(*id);
        }
    }
    let mut replies: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for id in doc.comments.keys() {
        let r = root(doc, *id);
        if r != *id {
            let v = replies.entry(r).or_default();
            if v.len() < MAX_REPLIES {
                v.push(*id);
            }
        }
    }
    let mut out = Vec::new();
    for id in order {
        let Some(c) = doc.comments.get(&id) else { continue };
        if root(doc, id) != id {
            continue;
        }
        let quote = quotes.get(&id).map(|q| q.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let quote = if quote.chars().count() > MAX_QUOTE { format!("{}…", quote.chars().take(MAX_QUOTE).collect::<String>()) } else { quote };
        let replies =
            replies.get(&id).into_iter().flatten().filter_map(|r| doc.comments.get(r)).map(|r| (author(&r.author), text_of(doc, r.part))).collect();
        out.push(Note { author: author(&c.author), quote, text: text_of(doc, c.part), replies });
        if out.len() >= MAX_COMMENTS {
            break;
        }
    }
    out
}

/// `bytes` (a copy in format `ext`) with the comments written at its end.
pub fn append(ext: &str, doc: &Document, bytes: Vec<u8>) -> Vec<u8> {
    let notes = notes(doc);
    if notes.is_empty() {
        return bytes;
    }
    match ext {
        "html" | "htm" | "xhtml" => {
            let mut s = match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(e) => return e.into_bytes(),
            };
            let at = s.rfind("</body>").unwrap_or(s.len());
            s.insert_str(at, &html(&notes));
            s.into_bytes()
        }
        "md" | "markdown" => {
            let mut s = bytes;
            s.extend_from_slice(markdown(&notes).as_bytes());
            s
        }
        "txt" | "text" => {
            let mut s = bytes;
            s.extend_from_slice(text(&notes).as_bytes());
            s
        }
        _ => bytes,
    }
}

fn html(notes: &[Note]) -> String {
    use crate::html::esc;
    let mut s = String::from("<section class=\"comments\">\n<h2>Comments</h2>\n<ol>\n");
    for (i, n) in notes.iter().enumerate() {
        s.push_str(&format!("<li id=\"comment-{}\"><p><strong>{}</strong>", i + 1, esc(&n.author)));
        if !n.quote.is_empty() {
            s.push_str(&format!(" on <q>{}</q>", esc(&n.quote)));
        }
        s.push_str(&format!(": {}</p>", esc(&n.text)));
        if !n.replies.is_empty() {
            s.push_str("<ul>");
            for (a, t) in &n.replies {
                s.push_str(&format!("<li><strong>{}</strong>: {}</li>", esc(a), esc(t)));
            }
            s.push_str("</ul>");
        }
        s.push_str("</li>\n");
    }
    s.push_str("</ol>\n</section>\n");
    s
}

fn markdown(notes: &[Note]) -> String {
    use crate::markdown::escape;
    let mut s = String::from("\n\n## Comments\n\n");
    for (i, n) in notes.iter().enumerate() {
        s.push_str(&format!("{}. **{}**", i + 1, escape(&n.author)));
        if !n.quote.is_empty() {
            s.push_str(&format!(" on “{}”", escape(&n.quote)));
        }
        s.push_str(&format!(": {}\n", escape(&n.text)));
        for (a, t) in &n.replies {
            s.push_str(&format!("    - **{}**: {}\n", escape(a), escape(t)));
        }
    }
    s
}

fn text(notes: &[Note]) -> String {
    let mut s = String::from("\r\n\r\nComments\r\n");
    for (i, n) in notes.iter().enumerate() {
        s.push_str(&format!("\r\n[{}] {}", i + 1, n.author));
        if !n.quote.is_empty() {
            s.push_str(&format!(" on “{}”", n.quote));
        }
        s.push_str(&format!(": {}\r\n", n.text));
        for (a, t) in &n.replies {
            s.push_str(&format!("    {a}: {t}\r\n"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcraft_doc::{CharProps, Comment, Paragraph, PartKind, para_block};

    #[test]
    fn multi_line_authors_stay_in_their_list_item() {
        let mut d = Document::from_text("Body text");
        let part = d.add_part(PartKind::Comment, vec![para_block(Paragraph::with_text("First line", CharProps::default()))]);
        d.comments.insert(0, Comment { author: "Ann\n\n# Heading\r\n2. Fake".into(), part, ..Default::default() });
        let part = d.add_part(PartKind::Comment, vec![para_block(Paragraph::with_text("Reply", CharProps::default()))]);
        d.comments.insert(1, Comment { author: "Bob\nSmith".into(), part, parent: Some(0), ..Default::default() });
        let n = notes(&d);
        assert_eq!(n[0].author, "Ann # Heading 2. Fake");
        assert_eq!(n[0].replies[0].0, "Bob Smith");
        let md = String::from_utf8(append("md", &d, Vec::new())).unwrap();
        let lines: Vec<&str> = md.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 3, "{md}");
        assert!(lines[1].starts_with("1. **Ann # Heading 2. Fake**"), "{md}");
        assert!(lines[2].starts_with("    - **Bob Smith**"), "{md}");
    }
}
