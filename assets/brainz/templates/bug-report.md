---
id: bug-report
name: Bug report
description: Repro steps, expected vs actual, environment and screenshots, ready for a GitHub issue or a fixing agent.
kind: note
icon: bug
group: Planning
modes: [new, insert]
path: '{{folder}}/bugs/{{date}}-{{slug input.title}}.md'
inputs:
  - { id: title, type: text, label: What's broken?, required: true }
  - { id: app, type: select, options: [Brainz, MyMan, Credo Chat iOS, Credo Chat web, Other], label: Which app? }
context: [selection]
---
# Bug: {{input.title}} ({{input.app}}) .. {{date}}

## Steps to reproduce
1. {{cursor}}

## Expected

## Actual
{{selection}}

## Environment
- App / version:
- Device / OS:

## Screenshots / logs

## Notes for the fixing agent
- Repo:
- Suspected area:
