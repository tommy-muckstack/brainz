---
id: daily
name: Daily note
description: Today's plan from the calendar and To-Do, plus a running log and an end-of-day check.
kind: note
icon: sun
group: Personal
path: 'journal/{{date "%Y"}}/{{date}}.md'
context: [calendar.today, todo.section.Today, todo.section.Urgent, open_loops]
---
# {{date "%A, %B %-d, %Y"}}

## Today's calendar
{{#each calendar.today}}
- {{date start "%-I:%M %p"}} {{title}}{{#if match}} .. [{{match.folder_name}}]({{link match.folder}}/CLAUDE.md){{#if match.prep_file}} · [prep]({{link match.prep_file}}){{/if}}{{/if}}
{{else}}
- No events.
{{/each}}

## Top 3
{{#ai "top3" context="calendar.today,todo.section.Today,todo.section.Urgent,open_loops" format="bullets" words=60}}
Pick the 3 most important things for today from the To-Do items, open loops and calendar. One line each, starting with a verb. Only use items from the context.
{{/ai}}

## Log
- {{cursor}}

## End of day
- Done:
- Slipped (and why):
- First thing tomorrow:
