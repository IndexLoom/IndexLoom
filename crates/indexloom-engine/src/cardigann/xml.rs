//! Strict XML-to-DOM adapter. No DTDs, external entities, HTML recovery or network IO.
use super::EngineError;
use markup5ever::{Attribute, QualName};
use quick_xml::{Reader, events::Event};
use scraper::{
    Html, Node,
    node::{Element, Text},
};
fn invalid() -> EngineError {
    EngineError::CardigannIncompatible(
        "invalid XML, unknown entity, DTD, or XML resource limit exceeded",
    )
}
#[allow(
    clippy::too_many_lines,
    reason = "One bounded XML event loop keeps tree and depth invariants together."
)]
pub(super) fn document(body: &str) -> Result<Html, EngineError> {
    let mut reader = Reader::from_str(body);
    let mut doc = Html::new_document();
    let mut stack = vec![doc.tree.root().id()];
    let mut nodes = 0;
    let mut roots = 0;
    loop {
        let event = reader.read_event().map_err(|_| invalid())?;
        nodes += 1;
        if nodes > 200_000 || stack.len() > 128 {
            return Err(invalid());
        }
        let parent = *stack.last().ok_or_else(invalid)?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                if stack.len() == 1 {
                    roots += 1;
                    if roots > 1 {
                        return Err(invalid());
                    }
                }
                let name = std::str::from_utf8(e.name().as_ref())
                    .map_err(|_| invalid())?
                    .to_owned();
                let mut attrs = Vec::new();
                for attr in e.attributes() {
                    let attr = attr.map_err(|_| invalid())?;
                    let key = std::str::from_utf8(attr.key.as_ref()).map_err(|_| invalid())?;
                    let value = attr
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(|_| invalid())?;
                    attrs.push(Attribute {
                        name: QualName::new(None, "".into(), key.into()),
                        value: value.as_ref().into(),
                    });
                }
                let id = doc
                    .tree
                    .get_mut(parent)
                    .ok_or_else(invalid)?
                    .append(Node::Element(Element::new(
                        QualName::new(None, "".into(), name.as_str().into()),
                        attrs,
                    )))
                    .id();
                if matches!(event, Event::Start(_)) {
                    stack.push(id);
                }
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return Err(invalid());
                }
                stack.pop();
            }
            Event::Text(e) => {
                let text = e
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|_| invalid())?;
                if stack.len() == 1 && !text.trim().is_empty() {
                    return Err(invalid());
                }
                doc.tree
                    .get_mut(parent)
                    .ok_or_else(invalid)?
                    .append(Node::Text(Text {
                        text: text.as_ref().into(),
                    }));
            }
            Event::CData(e) => {
                let text = e
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|_| invalid())?;
                doc.tree
                    .get_mut(parent)
                    .ok_or_else(invalid)?
                    .append(Node::Text(Text {
                        text: text.as_ref().into(),
                    }));
            }
            Event::GeneralRef(e) => {
                let text = e
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|_| invalid())?;
                let character = e
                    .resolve_char_ref()
                    .map_err(|_| invalid())?
                    .or_else(|| match text.as_ref() {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    })
                    .ok_or_else(invalid)?;
                doc.tree
                    .get_mut(parent)
                    .ok_or_else(invalid)?
                    .append(Node::Text(Text {
                        text: character.to_string().into(),
                    }));
            }
            Event::DocType(_) => return Err(invalid()),
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1 || roots != 1 {
        return Err(invalid());
    }
    Ok(doc)
}
