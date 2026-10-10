//! Reads a MudletPackage XML document (the C# `MudletPackageParser`). The document is
//! untrusted: no DTD is processed, no entity other than the five built-in ones and character
//! references is expanded (so nothing can multiply or be fetched), and sizes, item counts and
//! nesting are bounded.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::model::{ImportError, Item, ItemKind, Package, Pattern};
use crate::l10n::{S, tf};

/// Largest document, in bytes.
pub const MAX_DOCUMENT_BYTES: usize = 48 * 1024 * 1024;
/// Most items (folders included) in one document.
pub const MAX_ITEMS: usize = 20_000;
/// Deepest folder nesting.
pub const MAX_DEPTH: usize = 48;
/// Longest text of one field, in characters.
pub const MAX_TEXT_CHARACTERS: usize = 2 * 1024 * 1024;
/// Deepest element nesting the reader accepts at all (far beyond any Mudlet file).
const MAX_ELEMENT_DEPTH: usize = 4 * MAX_DEPTH + 16;

/// One element of the document, with its text (character data directly inside it).
#[derive(Default)]
struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    text: String,
    children: Vec<Element>,
}

impl Element {
    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// The element's text with that of everything inside it, as .NET's `XElement.Value`.
    fn value(&self) -> String {
        let mut out = self.text.clone();
        for child in &self.children {
            out.push_str(&child.value());
        }
        out
    }

    fn descendants_named(&self, names: &[&str]) -> usize {
        self.children
            .iter()
            .map(|c| usize::from(names.contains(&c.name.as_str())) + c.descendants_named(names))
            .sum()
    }
}

fn not_mudlet(detail: &str) -> ImportError {
    ImportError(tf(S::MudletImportNotMudletXml, &[&detail]))
}

fn local(name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name);
    match name.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => name.into_owned(),
    }
}

fn start(element: &BytesStart<'_>) -> Result<Element, ImportError> {
    let mut attributes = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|e| not_mudlet(&e.to_string()))?;
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|e| not_mudlet(&e.to_string()))?;
        attributes.push((local(attribute.key.as_ref()), value.into_owned()));
    }
    Ok(Element {
        name: local(element.name().as_ref()),
        attributes,
        ..Element::default()
    })
}

/// Parse the whole document into elements. Undefined entities fail; the DOCTYPE is ignored.
fn document(xml: &[u8]) -> Result<Element, ImportError> {
    if xml.len() > MAX_DOCUMENT_BYTES {
        return Err(ImportError::new(S::MudletImportTooLarge));
    }
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().check_end_names = true;
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut buffer = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|e| not_mudlet(&e.to_string()))?;
        match event {
            Event::Start(element) => {
                if root.is_some() {
                    return Err(not_mudlet("content after the root element"));
                }
                if stack.len() >= MAX_ELEMENT_DEPTH {
                    return Err(ImportError::new(S::MudletImportTooDeep));
                }
                stack.push(start(&element)?);
            }
            Event::End(_) => {
                let Some(done) = stack.pop() else {
                    return Err(not_mudlet("unbalanced end tag"));
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => root = Some(done),
                }
            }
            Event::Text(text) => {
                let content = text.xml10_content().map_err(|e| not_mudlet(&e.to_string()))?;
                push_text(&mut stack, &content)?;
            }
            Event::CData(data) => {
                let content = data.decode().map_err(|e| not_mudlet(&e.to_string()))?;
                push_text(&mut stack, &content)?;
            }
            Event::GeneralRef(reference) => {
                let resolved = if reference.is_char_ref() {
                    reference
                        .resolve_char_ref()
                        .map_err(|e| not_mudlet(&e.to_string()))?
                        .map(String::from)
                } else {
                    let name = reference.decode().map_err(|e| not_mudlet(&e.to_string()))?;
                    match name.as_ref() {
                        "lt" => Some("<".into()),
                        "gt" => Some(">".into()),
                        "amp" => Some("&".into()),
                        "apos" => Some("'".into()),
                        "quot" => Some("\"".into()),
                        // Any other entity would come from a DTD, which is never processed.
                        other => return Err(not_mudlet(&format!("&{other};"))),
                    }
                };
                let Some(resolved) = resolved else {
                    return Err(not_mudlet("character reference"));
                };
                push_text(&mut stack, &resolved)?;
            }
            Event::Eof => break,
            // The declaration, comments, processing instructions and the DOCTYPE carry nothing.
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err(not_mudlet("unclosed element"));
    }
    root.ok_or_else(|| not_mudlet(""))
}

fn push_text(stack: &mut [Element], text: &str) -> Result<(), ImportError> {
    if let Some(top) = stack.last_mut() {
        top.text.push_str(text);
        if top.text.len() > MAX_TEXT_CHARACTERS * 4 {
            return Err(ImportError::new(S::MudletImportTooLarge));
        }
    }
    Ok(())
}

/// Parse one MudletPackage document.
pub fn parse(xml: &[u8]) -> Result<Package, ImportError> {
    let root = document(xml)?;
    if root.name != "MudletPackage" {
        return Err(not_mudlet(&root.name));
    }
    let mut budget = 0usize;
    let host = root.child("HostPackage").and_then(|h| h.child("Host"));
    let separator = match host {
        None => ";;".to_string(),
        Some(host) => match host
            .child("mCommandSeparator")
            .or_else(|| host.child("commandSeperator"))
        {
            Some(e) => clip(e.value())?,
            None => ";;".to_string(),
        },
    };
    let port = host.and_then(|h| text(h, "port").ok()).and_then(|p| {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            None
        } else {
            p.parse::<u16>().ok()
        }
    });
    let tls = host.and_then(|h| h.attribute("mSslTsl")).map(|v| v == "yes");
    let modules = match host.and_then(|h| h.child("mInstalledModules")) {
        Some(modules) => modules
            .children_named("key")
            .map(|e| clip(e.value()))
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    Ok(Package {
        has_host: host.is_some(),
        host_name: host.map(|h| text(h, "name")).transpose()?.unwrap_or_default(),
        url: host.map(|h| text(h, "url")).transpose()?.unwrap_or_default(),
        port,
        tls,
        command_separator: separator,
        modules,
        triggers: items(&root, "TriggerPackage", ItemKind::Trigger, &mut budget)?,
        aliases: items(&root, "AliasPackage", ItemKind::Alias, &mut budget)?,
        timers: items(&root, "TimerPackage", ItemKind::Timer, &mut budget)?,
        keys: items(&root, "KeyPackage", ItemKind::Key, &mut budget)?,
        scripts: items(&root, "ScriptPackage", ItemKind::Script, &mut budget)?,
        buttons: items(&root, "ActionPackage", ItemKind::Button, &mut budget)?,
        variable_count: root
            .child("VariablePackage")
            .map_or(0, |v| v.descendants_named(&["Variable", "VariableGroup"])),
    })
}

fn names(kind: ItemKind) -> (&'static str, &'static str) {
    match kind {
        ItemKind::Trigger => ("Trigger", "TriggerGroup"),
        ItemKind::Alias => ("Alias", "AliasGroup"),
        ItemKind::Timer => ("Timer", "TimerGroup"),
        ItemKind::Key => ("Key", "KeyGroup"),
        ItemKind::Script => ("Script", "ScriptGroup"),
        ItemKind::Button => ("Action", "ActionGroup"),
    }
}

fn items(root: &Element, package: &str, kind: ItemKind, budget: &mut usize) -> Result<Vec<Item>, ImportError> {
    match root.child(package) {
        Some(container) => children(container, kind, budget, 0),
        None => Ok(Vec::new()),
    }
}

fn children(parent: &Element, kind: ItemKind, budget: &mut usize, depth: usize) -> Result<Vec<Item>, ImportError> {
    let (item_name, group_name) = names(kind);
    let mut out = Vec::new();
    for element in &parent.children {
        if element.name != item_name && element.name != group_name {
            continue;
        }
        if depth >= MAX_DEPTH {
            return Err(ImportError::new(S::MudletImportTooDeep));
        }
        *budget += 1;
        if *budget > MAX_ITEMS {
            return Err(ImportError::new(S::MudletImportTooManyItems));
        }
        out.push(item(element, kind, element.name == group_name, budget, depth)?);
    }
    Ok(out)
}

fn item(element: &Element, kind: ItemKind, group: bool, budget: &mut usize, depth: usize) -> Result<Item, ImportError> {
    let mut item = Item::new(kind);
    if kind == ItemKind::Trigger {
        let texts: Vec<String> = match element.child("regexCodeList") {
            Some(list) => list
                .children_named("string")
                .map(|e| clip(e.value()))
                .collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let types: Vec<i32> = element
            .child("regexCodePropertyList")
            .map(|list| {
                list.children_named("integer")
                    .map(|e| e.value().trim().parse::<i32>().unwrap_or(-1))
                    .collect()
            })
            .unwrap_or_default();
        if texts.len() > 256 {
            return Err(ImportError::new(S::MudletImportTooManyItems));
        }
        item.patterns = texts
            .into_iter()
            .enumerate()
            .map(|(i, text)| Pattern::new(types.get(i).copied().unwrap_or(-1), text))
            .collect();
    }
    item.command = match kind {
        ItemKind::Trigger => text(element, "mCommand")?,
        ItemKind::Button => text(element, "commandButtonUp")?,
        _ => text(element, "command")?,
    };
    item.name = text(element, "name")?;
    item.is_folder = group || flag(element, "isFolder");
    item.is_active = flag(element, "isActive");
    item.script = text(element, "script")?;
    if kind == ItemKind::Alias {
        item.alias_pattern = text(element, "regex")?;
    }
    if kind == ItemKind::Timer {
        item.time = text(element, "time")?;
    }
    if kind == ItemKind::Key {
        item.key_code = number(element, "keyCode")?;
        item.key_modifier = number(element, "keyModifier")?;
    }
    if kind == ItemKind::Script
        && let Some(list) = element.child("eventHandlerList")
    {
        item.event_handlers = list
            .children_named("string")
            .take(256)
            .map(|e| clip(e.value()))
            .collect::<Result<_, _>>()?;
    }
    item.is_multiline = flag(element, "isMultiline");
    item.is_filter = flag(element, "isFilterTrigger");
    item.is_highlight = flag(element, "isColorizerTrigger");
    item.is_sound = flag(element, "isSoundTrigger");
    item.is_colour_trigger = flag(element, "isColorTrigger");
    item.is_temporary = flag(element, "isTempTrigger") || flag(element, "isTempTimer");
    item.is_offset_timer = flag(element, "isOffsetTimer");
    item.children = children(element, kind, budget, depth + 1)?;
    Ok(item)
}

fn flag(element: &Element, attribute: &str) -> bool {
    element.attribute(attribute) == Some("yes")
}

fn number(element: &Element, child: &str) -> Result<i64, ImportError> {
    Ok(text(element, child)?.trim().parse::<i64>().unwrap_or(0))
}

fn text(element: &Element, child: &str) -> Result<String, ImportError> {
    match element.child(child) {
        Some(e) => clip(e.value()),
        None => Ok(String::new()),
    }
}

/// A field longer than the limit makes the whole file too large to import.
fn clip(value: String) -> Result<String, ImportError> {
    if value.len() > MAX_TEXT_CHARACTERS && value.encode_utf16().count() > MAX_TEXT_CHARACTERS {
        return Err(ImportError::new(S::MudletImportTooLarge));
    }
    Ok(value)
}
