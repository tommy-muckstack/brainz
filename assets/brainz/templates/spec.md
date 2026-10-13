---
id: spec
name: Spec / PRD
description: Problem, users, goals, requirements with acceptance criteria, and open questions. Built to hand to a coding agent.
kind: note
icon: doc
group: Planning
path: '{{folder}}/{{slug input.title}}-spec.md'
inputs:
  - { id: title, type: text, label: What are we building?, required: true }
context: [selection, related]
---
# {{input.title}}: spec

> **Status {{date}}:** draft. Owner {{owner}}.

## Problem
{{#ai "problem" context="selection,related" words=120}}
State the problem in 2 to 4 sentences using only the selection and related notes. Say who has it and what it costs them.
{{/ai}}

## Users and user stories
- As ..., I want ..., so that ...

## Goals / non-goals
- Goal:
- Non-goal:

## Requirements
| # | Requirement | Priority | Acceptance criteria |
|---|---|---|---|
| R1 | {{cursor}} | P0 | |

## UX notes

## Implementation notes (files, APIs)

## Test plan

## Open questions
1.
