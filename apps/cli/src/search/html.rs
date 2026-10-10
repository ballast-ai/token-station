//! Incremental HTML tokenization with bounded structure and interruptible extraction.
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
    states::RawKind,
};
use std::cell::{Cell, RefCell};

const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 50_000;
const MAX_TOKEN_BYTES: usize = 16_384;
const CHUNK_BYTES: usize = 1024;

#[derive(Default)]
pub(super) struct Node {
    pub name: String,
    pub class: String,
    pub href: Option<String>,
    text: String,
    end: usize,
    parent: Option<usize>,
}

#[derive(Default)]
pub(super) struct Document {
    nodes: Vec<Node>,
    stack: Vec<usize>,
}

impl Document {
    fn close_from(&mut self, position: usize) {
        for index in self.stack.drain(position..) {
            self.nodes[index].end = self.nodes.len();
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn node(&self, index: usize) -> &Node {
        &self.nodes[index]
    }

    pub fn end(&self, index: usize) -> usize {
        self.nodes[index].end
    }

    pub fn first(
        &self,
        start: usize,
        end: usize,
        predicate: impl Fn(&Node) -> bool,
        check: &dyn Fn() -> Result<(), String>,
    ) -> Result<Option<usize>, String> {
        for index in start..end {
            check()?;
            if predicate(&self.nodes[index]) {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    pub fn text(
        &self,
        start: usize,
        end: usize,
        limit: usize,
        excluded: &[&str],
        check: &dyn Fn() -> Result<(), String>,
    ) -> Result<(String, bool), String> {
        // A selected main/article can itself be inside a discarded region.
        let mut parent = self.nodes.get(start).and_then(|node| node.parent);
        while let Some(index) = parent {
            check()?;
            if excluded.contains(&self.nodes[index].name.as_str()) {
                return Ok((String::new(), false));
            }
            parent = self.nodes[index].parent;
        }
        let mut output = String::new();
        let mut count = 0;
        let mut space = false;
        let mut index = start;
        while index < end {
            check()?;
            let node = &self.nodes[index];
            if excluded.contains(&node.name.as_str()) {
                index = node.end;
                continue;
            }
            for character in node.text.chars() {
                check()?;
                if character.is_whitespace() {
                    space = !output.is_empty();
                    continue;
                }
                for ch in space
                    .then_some(' ')
                    .into_iter()
                    .chain(std::iter::once(character))
                {
                    if count == limit {
                        return Ok((output, true));
                    }
                    output.push(ch);
                    count += 1;
                }
                space = false;
            }
            if !node.text.is_empty() {
                space = !output.is_empty();
            }
            index += 1;
        }
        Ok((output, false))
    }
}

pub(super) fn has_class(node: &Node, class: &str) -> bool {
    node.class
        .split_ascii_whitespace()
        .any(|value| value == class)
}

fn close_omitted(document: &mut Document, name: &str) {
    if name == "li" {
        close_in_scope(
            document,
            "li",
            &["ul", "ol", "html", "table", "td", "th", "template"],
        );
    }
    if matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "center"
            | "details"
            | "dialog"
            | "dir"
            | "div"
            | "dl"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "header"
            | "hgroup"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "p"
            | "search"
            | "section"
            | "summary"
            | "ul"
            | "pre"
            | "listing"
            | "table"
            | "hr"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "li"
    ) {
        close_in_scope(
            document,
            "p",
            &[
                "button", "applet", "caption", "html", "table", "td", "th", "marquee", "object",
                "template", "svg", "math",
            ],
        );
    }
}

fn close_in_scope(document: &mut Document, name: &str, boundaries: &[&str]) {
    let position = document
        .stack
        .iter()
        .enumerate()
        .rev()
        .take_while(|(_, index)| !boundaries.contains(&document.nodes[**index].name.as_str()))
        .find(|(_, index)| document.nodes[**index].name == name)
        .map(|(position, _)| position);
    if let Some(position) = position {
        document.close_from(position);
    }
}

struct Sink<'a> {
    document: RefCell<Document>,
    error: RefCell<Option<String>>,
    check: &'a dyn Fn() -> Result<(), String>,
    tokens: Cell<usize>,
}

impl Sink<'_> {
    fn start_tag(document: &mut Document, tag: &Tag) -> Result<TokenSinkResult<()>, String> {
        let name = tag.name.to_string();
        close_omitted(document, &name);
        if document.nodes.len() >= MAX_NODES {
            return Err("HTML exceeded the node limit.".into());
        }
        let void = matches!(
            name.as_str(),
            "area"
                | "base"
                | "br"
                | "col"
                | "embed"
                | "hr"
                | "img"
                | "input"
                | "link"
                | "meta"
                | "param"
                | "source"
                | "track"
                | "wbr"
        );
        if !void && document.stack.len() >= MAX_DEPTH {
            return Err("HTML exceeded the depth limit.".into());
        }
        let raw = match name.as_str() {
            "script" => Some(RawKind::ScriptData),
            "style" | "xmp" | "iframe" | "noembed" | "noframes" => Some(RawKind::Rawtext),
            "title" | "textarea" => Some(RawKind::Rcdata),
            _ => None,
        };
        let class = tag
            .attrs
            .iter()
            .find(|attr| attr.name.local.as_ref() == "class")
            .map(|attr| attr.value.to_string())
            .unwrap_or_default();
        let href = tag
            .attrs
            .iter()
            .find(|attr| attr.name.local.as_ref() == "href")
            .map(|attr| attr.value.to_string());
        let index = document.nodes.len();
        let parent = document.stack.last().copied();
        document.nodes.push(Node {
            parent,
            name,
            class,
            href,
            text: String::new(),
            end: index + 1,
        });
        // HTML non-void self-closing syntax does not close an HTML element.
        if !void {
            document.stack.push(index);
        }
        if let Some(raw) = raw {
            return Ok(TokenSinkResult::RawData(raw));
        }
        Ok(TokenSinkResult::Continue)
    }

    fn process(&self, token: Token) -> Result<TokenSinkResult<()>, String> {
        (self.check)()?;
        // Parse errors are emitted even while an incomplete tag accumulates.
        // They must not reset the incomplete-token byte budget.
        if !matches!(token, Token::ParseError(_)) {
            self.tokens.set(self.tokens.get() + 1);
        }
        let mut document = self.document.borrow_mut();
        match token {
            Token::TagToken(tag) if tag.kind == TagKind::StartTag => {
                return Self::start_tag(&mut document, &tag);
            }
            Token::TagToken(tag) => {
                if let Some(position) = document
                    .stack
                    .iter()
                    .rposition(|index| document.nodes[*index].name == tag.name.as_ref())
                {
                    document.close_from(position);
                }
            }
            Token::CharacterTokens(text) => {
                let parent = document.stack.last().copied();
                // Tokenizer feeds and character references can split one DOM text
                // run. Merge adjacent runs so chunking never adds word spaces.
                if let Some(previous) = document.nodes.last_mut()
                    && previous.name.is_empty()
                    && previous.parent == parent
                {
                    previous.text.push_str(&text);
                    return Ok(TokenSinkResult::Continue);
                }
                if document.nodes.len() >= MAX_NODES {
                    return Err("HTML exceeded the node limit.".into());
                }
                let index = document.nodes.len();
                let parent = document.stack.last().copied();
                document.nodes.push(Node {
                    parent,
                    text: text.to_string(),
                    end: index + 1,
                    ..Node::default()
                });
            }
            _ => {}
        }
        Ok(TokenSinkResult::Continue)
    }
}

impl TokenSink for Sink<'_> {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        if self.error.borrow().is_some() {
            return TokenSinkResult::Continue;
        }
        match self.process(token) {
            Ok(result) => result,
            Err(error) => {
                *self.error.borrow_mut() = Some(error);
                // Character, parse-error, and EOF callbacks require Continue.
                // Stop writes now. parse returns this error after the bounded feed.
                TokenSinkResult::Continue
            }
        }
    }
}

pub(super) fn parse(
    input: &str,
    max_bytes: usize,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<Document, String> {
    check()?;
    if input.len() > max_bytes {
        return Err("HTML exceeded the input byte limit.".into());
    }
    let tokenizer = Tokenizer::new(
        Sink {
            document: RefCell::default(),
            error: RefCell::default(),
            check,
            tokens: Cell::new(0),
        },
        TokenizerOpts::default(),
    );
    let queue = BufferQueue::default();
    let mut offset = 0;
    let mut pending_bytes = 0;
    while offset < input.len() {
        check()?;
        let mut end = (offset + CHUNK_BYTES).min(input.len());
        while !input.is_char_boundary(end) {
            end -= 1;
        }
        let before = tokenizer.sink.tokens.get();
        queue.push_back(StrTendril::from_slice(&input[offset..end]));
        let _ = tokenizer.feed(&queue);
        if let Some(error) = tokenizer.sink.error.borrow_mut().take() {
            return Err(error);
        }
        // Bound a tag/comment/entity that is incomplete across feeds, before it
        // can accumulate an excessive attribute list or tokenizer buffer.
        pending_bytes = if tokenizer.sink.tokens.get() == before {
            pending_bytes + end - offset
        } else {
            end - offset
        };
        if pending_bytes > MAX_TOKEN_BYTES {
            return Err("HTML exceeded the token byte limit.".into());
        }
        offset = end;
    }
    check()?;
    tokenizer.end();
    if let Some(error) = tokenizer.sink.error.borrow_mut().take() {
        return Err(error);
    }
    let mut document = tokenizer.sink.document.into_inner();
    document.close_from(0);
    check()?;
    Ok(document)
}
