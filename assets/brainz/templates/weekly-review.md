---
id: weekly-review
name: Weekly review
description: Friday review of what moved, what slipped, open loops, stale status lines and next week's top priorities.
kind: note
icon: review
group: Reviews
path: 'reviews/{{date "%G"}}-W{{date "%V"}}.md'
context: [git.changes, todo.open, open_loops, stale, myman.meetings, calendar.next_week]
---
# Week {{date "%V"}} review .. {{date "monday" "%b %-d"}} to {{date "%b %-d, %Y"}}

## What moved
{{#ai "moved" context="git.changes,myman.meetings" format="bullets" words=150}}
Summarize what changed in the brain this week, grouped by project folder. One line per project, with the most important file linked.
{{/ai}}

## What slipped
{{#ai "slipped" context="todo.open,open_loops" format="bullets" words=100}}
List To-Do items and open loops that are overdue or older than 7 days. Keep their tags.
{{/ai}}

## Stale status lines
{{#each stale}}
- [{{folder}}]({{link folder}}/CLAUDE.md) .. status {{status_date}}, newest note {{newest_date}}
{{else}}
- None.
{{/each}}

## Next week
{{#each calendar.next_week}}
- {{date start "%a %-I:%M %p"}} {{title}}
{{/each}}

## Top 3 for next week
1. {{cursor}}
2.
3.
