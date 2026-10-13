---
id: meeting-prep
name: Meeting prep
description: Prep note for an upcoming call, with attendees, last-call recap, a drafted agenda and sharp questions.
kind: note
icon: calendar
group: Meetings
tags: [meeting, prep, call]
modes: [new]
path: '{{#if match}}{{match.folder}}/{{date event.start}}/{{slug match.who}}-prep.md{{else}}meetings/{{date event.start}}-{{slug event.title}}-prep.md{{/if}}'
applies_to: ["**/*-prep.md"]
inputs:
  - { id: event, type: calendar.event, label: Which event?, default: calendar.current_or_next, required: true }
context: [match, people, related, last_meeting, myman.last_transcript, todo.open]
---
# {{default match.who event.title}} .. {{date event.start "%Y-%m-%d, %-I:%M %p"}} ET .. prep

**When:** {{date event.start "%a %b %-d, %-I:%M %p"}} ({{event.calendar}}){{#if event.location}} · {{event.location}}{{/if}}
**Who:** {{#if people}}{{#each people}}[{{name}}]({{link file}}){{#unless @last}}, {{/unless}}{{/each}}{{else}}{{join event.attendee_names ", "}}{{/if}}
{{#if match}}**Folder:** [{{match.folder_name}}]({{link match.folder}}/CLAUDE.md){{/if}}
{{#if last_meeting}}**Last time:** [{{last_meeting.title}}]({{link last_meeting.path}}) ({{last_meeting.date}}){{/if}}

## Goal
{{cursor}}

## Agenda
{{#ai "agenda" context="event,related,last_meeting" format="bullets" words=120}}
Draft 3 to 5 agenda bullets for this call, ordered by importance. Use only facts from the context.
{{/ai}}

## Questions to ask
{{#ai "questions" context="event,related,last_meeting,myman.last_transcript" format="bullets" words=100}}
List 3 to 5 sharp questions for this call. Prefer open threads from the last meeting and the folder's status callout. No generic questions.
{{/ai}}

## Open threads from last time
{{#ai "threads" context="last_meeting,myman.last_transcript,todo.open" format="bullets" words=80}}
List commitments either side made last time that are still open. Cite the note or transcript for each. If none, say "None found."
{{/ai}}

## After the call
- Review decisions and add follow-up tasks.
