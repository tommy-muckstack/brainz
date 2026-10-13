---
id: meeting
name: Meeting notes
description: Notes for a meeting happening now or just finished, with attendees, decisions and actions.
kind: note
icon: calendar
group: Meetings
modes: [new, insert]
path: '{{#if match}}{{match.folder}}/{{date}}/{{slug event.title}}.md{{else}}meetings/{{date}}-{{slug event.title}}.md{{/if}}'
applies_to: ["**/meetings/**"]
inputs:
  - { id: event, type: calendar.event, label: Which meeting?, default: calendar.current_or_next }
context: [match, people, myman.last_transcript]
---
# {{event.title}} .. {{date event.start "%Y-%m-%d"}}

**When:** {{date event.start "%-I:%M %p"}} to {{date event.end "%-I:%M %p"}} ET
**Who:** {{#if people}}{{#each people}}[{{name}}]({{link file}}){{#unless @last}}, {{/unless}}{{/each}}{{else}}{{join event.attendee_names ", "}}{{/if}}
{{#if event.url}}**Link:** {{event.url}}{{/if}}

## Notes
- {{cursor}}

## Decisions
-

## Actions
- [ ] 

## Sources
{{#if myman.last_transcript}}- My Man transcript: [{{myman.last_transcript.title}}]({{myman.last_transcript.path}}){{/if}}
