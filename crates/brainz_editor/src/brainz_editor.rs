use std::{fmt, ops::Range, sync::Arc};

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    Quote,
    List(Option<u64>),
    Item,
    Code(Option<String>),
    Html,
    Table,
    TableHead,
    TableRow,
    TableCell,
    Metadata,
    Footnote,
    Divider,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub range: Range<usize>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

/// A transient projection of one buffer snapshot. Byte ranges are not stable
/// identities: the UI must convert them to buffer anchors before retaining them.
pub struct BlockMap {
    source: Arc<str>,
    blocks: Vec<Block>,
    roots: Vec<usize>,
}

const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_SMART_PUNCTUATION)
    .union(Options::ENABLE_HEADING_ATTRIBUTES)
    .union(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS)
    .union(Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS)
    .union(Options::ENABLE_OLD_FOOTNOTES)
    .union(Options::ENABLE_GFM)
    .union(Options::ENABLE_SUPERSCRIPT)
    .union(Options::ENABLE_SUBSCRIPT);

impl BlockMap {
    pub fn parse(source: impl Into<Arc<str>>) -> Self {
        let source = source.into();
        let mut blocks: Vec<Block> = Vec::new();
        let mut roots = Vec::new();
        let mut stack: Vec<Option<usize>> = Vec::new();
        for (event, range) in Parser::new_ext(&source, OPTIONS).into_offset_iter() {
            let (kind, opens_tag) = match event {
                Event::Start(tag) => (block_kind(tag), true),
                Event::End(_) => {
                    stack.pop();
                    continue;
                }
                Event::Rule => (Some(BlockKind::Divider), false),
                _ => continue,
            };
            let index = kind.map(|kind| {
                let parent = stack.iter().rev().find_map(|index| *index);
                let index = blocks.len();
                if let Some(parent_block) = parent.and_then(|parent| blocks.get_mut(parent)) {
                    parent_block.children.push(index);
                } else {
                    roots.push(index);
                }
                blocks.push(Block {
                    kind,
                    range,
                    parent,
                    children: Vec::new(),
                });
                index
            });
            if opens_tag {
                stack.push(index);
            }
        }
        Self {
            source,
            blocks,
            roots,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn roots(&self) -> &[usize] {
        &self.roots
    }

    pub fn text(&self, block: usize) -> Option<&str> {
        self.source.get(self.blocks.get(block)?.range.clone())
    }

    /// Includes whitespace and reference definitions that Markdown parsers do
    /// not emit as blocks, so a renderer never owns or normalizes those bytes.
    pub fn source_segments(&self) -> impl Iterator<Item = &str> {
        let mut previous_end = 0;
        let mut ranges = Vec::new();
        for index in &self.roots {
            if let Some(block) = self.blocks.get(*index) {
                if previous_end < block.range.start {
                    ranges.push(previous_end..block.range.start);
                }
                ranges.push(block.range.clone());
                previous_end = block.range.end;
            }
        }
        if previous_end < self.source.len() {
            ranges.push(previous_end..self.source.len());
        }
        ranges
            .into_iter()
            .filter_map(|range| self.source.get(range))
    }

    pub fn deepest_block_at(&self, offset: usize) -> Option<usize> {
        self.blocks
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, block)| block.range.contains(&offset).then_some(index))
    }

    pub fn replace(
        &self,
        range: Range<usize>,
        replacement: impl Into<String>,
    ) -> Result<EditPlan, EditError> {
        self.plan(vec![SourceEdit {
            range,
            replacement: replacement.into(),
        }])
    }

    /// Changes only the requested delimiter pair. Existing alternatives such as
    /// underscores elsewhere in the paragraph remain untouched.
    pub fn toggle_bold(&self, range: Range<usize>) -> Result<EditPlan, EditError> {
        if range.is_empty() || self.source.get(range.clone()).is_none() {
            return Err(EditError::InvalidRange);
        }
        let before = range
            .start
            .checked_sub(2)
            .and_then(|start| self.source.get(start..range.start));
        let after = range
            .end
            .checked_add(2)
            .and_then(|end| self.source.get(range.end..end));
        if before == Some("**") && after == Some("**") {
            self.plan(vec![
                SourceEdit {
                    range: range.start - 2..range.start,
                    replacement: String::new(),
                },
                SourceEdit {
                    range: range.end..range.end + 2,
                    replacement: String::new(),
                },
            ])
        } else {
            self.plan(vec![
                SourceEdit {
                    range: range.start..range.start,
                    replacement: "**".into(),
                },
                SourceEdit {
                    range: range.end..range.end,
                    replacement: "**".into(),
                },
            ])
        }
    }

    fn plan(&self, edits: Vec<SourceEdit>) -> Result<EditPlan, EditError> {
        let mut previous_end = 0;
        for edit in &edits {
            if edit.range.start < previous_end || self.source.get(edit.range.clone()).is_none() {
                return Err(EditError::InvalidRange);
            }
            previous_end = edit.range.end;
        }
        Ok(EditPlan {
            base: self.source.clone(),
            edits,
        })
    }
}

fn block_kind(tag: Tag<'_>) -> Option<BlockKind> {
    Some(match tag {
        Tag::Paragraph => BlockKind::Paragraph,
        Tag::Heading { level, .. } => BlockKind::Heading(level as u8),
        Tag::BlockQuote(_) => BlockKind::Quote,
        Tag::List(start) => BlockKind::List(start),
        Tag::Item => BlockKind::Item,
        Tag::CodeBlock(kind) => BlockKind::Code(match kind {
            CodeBlockKind::Fenced(language) if !language.is_empty() => Some(language.into_string()),
            _ => None,
        }),
        Tag::HtmlBlock => BlockKind::Html,
        Tag::Table(_) => BlockKind::Table,
        Tag::TableHead => BlockKind::TableHead,
        Tag::TableRow => BlockKind::TableRow,
        Tag::TableCell => BlockKind::TableCell,
        Tag::MetadataBlock(_) => BlockKind::Metadata,
        Tag::FootnoteDefinition(_) => BlockKind::Footnote,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEdit {
    pub range: Range<usize>,
    pub replacement: String,
}

pub struct EditPlan {
    base: Arc<str>,
    edits: Vec<SourceEdit>,
}

impl EditPlan {
    pub fn replace(
        base: impl Into<Arc<str>>,
        range: Range<usize>,
        replacement: impl Into<String>,
    ) -> Result<Self, EditError> {
        let base = base.into();
        if base.get(range.clone()).is_none() {
            return Err(EditError::InvalidRange);
        }
        Ok(Self {
            base,
            edits: vec![SourceEdit {
                range,
                replacement: replacement.into(),
            }],
        })
    }

    /// Validate against the current buffer immediately before applying the
    /// returned edits in one buffer transaction, without an intervening await.
    pub fn edits_for(&self, current: &str) -> Result<&[SourceEdit], EditError> {
        if current != self.base.as_ref() {
            return Err(EditError::ChangedSource);
        }
        Ok(&self.edits)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditError {
    InvalidRange,
    ChangedSource,
}

impl fmt::Display for EditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRange => "The selection is not a valid text range.",
            Self::ChangedSource => "The note changed. Make the selection again before editing.",
        })
    }
}

impl std::error::Error for EditError {}

#[cfg(test)]
mod tests;
