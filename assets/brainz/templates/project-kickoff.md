---
id: project-kickoff
name: Project kickoff
description: New project folder CLAUDE.md with a status callout, goal, people, milestones and risks.
kind: note
icon: rocket
group: Planning
modes: [new]
path: '{{folder}}/{{slug input.title}}/CLAUDE.md'
inputs:
  - { id: title, type: text, label: Project name, required: true }
  - { id: goal, type: longtext, label: One-line goal, required: true }
context: [related]
---
# {{input.title}}

> **Status {{date}}:** kicked off. Next: {{cursor}}

{{input.goal}}

## Why now

## People
-

## Milestones
| Date | Milestone | Done when |
|---|---|---|
| {{date "+14d"}} | | |

## Risks
{{#ai "risks" context="related" format="bullets" words=80}}
List the 3 most likely risks for this project given the goal and any related notes. One line each.
{{/ai}}

## Notes (chronological)
- {{date}} .. kickoff.
