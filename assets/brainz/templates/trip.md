---
id: trip
name: Trip
description: Trip plan with dates, bookings, a day-by-day itinerary and packing.
kind: note
icon: plane
group: Personal
path: 'trips/{{date input.start "%Y-%m"}}-{{slug input.destination}}/itinerary.md'
inputs:
  - { id: destination, type: text, label: Where?, required: true }
  - { id: start, type: date, label: Leaving, required: true }
  - { id: end, type: date, label: Back, required: true }
---
# {{input.destination}} .. {{date input.start "%b %-d"}} to {{date input.end "%b %-d, %Y"}}

## Bookings
| What | Confirmation | When | Cost |
|---|---|---|---|
| Flight out | | {{input.start}} | |
| Lodging | | | |
| Flight back | | {{input.end}} | |

## Day by day
- **{{date input.start "%a %b %-d"}}:** {{cursor}}

## Packing
- [ ] Chargers
- [ ] IDs

## Before we go
- [ ] Hold mail
- [ ] Pets / plants
