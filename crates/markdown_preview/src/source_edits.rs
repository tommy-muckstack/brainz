use anyhow::{Result, ensure};
use brainz_editor::EditPlan;
use editor::{Editor, MultiBufferOffset};
use gpui::{App, Entity, Window};

pub(crate) fn apply(
    editor: &Entity<Editor>,
    plan: &EditPlan,
    window: &mut Window,
    cx: &mut App,
) -> Result<()> {
    editor.update(cx, |editor, cx| {
        ensure!(!editor.read_only(cx), "This note is read-only.");
        let source = editor.text(cx);
        let edits = plan.edits_for(&source)?;
        if edits
            .iter()
            .all(|edit| source.get(edit.range.clone()) == Some(edit.replacement.as_str()))
        {
            return Ok(());
        }
        editor.finalize_last_transaction(cx);
        let transaction = editor.transact(window, cx, |editor, _, cx| {
            editor.edit(
                edits.iter().map(|edit| {
                    (
                        MultiBufferOffset(edit.range.start)..MultiBufferOffset(edit.range.end),
                        edit.replacement.clone(),
                    )
                }),
                cx,
            );
        });
        editor.finalize_last_transaction(cx);
        ensure!(transaction.is_some(), "The note could not be edited.");
        Ok(())
    })
}
