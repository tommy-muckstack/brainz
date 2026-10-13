---
id: one-on-one
name: 1:1
description: Recurring 1:1 note that carries forward open items from the last one with this person.
kind: note
icon: people
group: Meetings
path: '1on1/{{slug input.person.name}}/{{date}}.md'
inputs:
  - { id: person, type: person, label: With who?, default: calendar.next.people.0 }
context: [last_meeting, todo.open]
---
# 1:1 with {{input.person.name}} .. {{date}}

## Carried over
{{#ai "carry" context="last_meeting,todo.open" format="bullets" words=80}}
List the unchecked items and open questions from the last 1:1 with this person. Keep their wording.
{{/ai}}

## Their topics
-

## My topics
- {{cursor}}

## Feedback (both ways)
-

## Actions
- [ ] 
