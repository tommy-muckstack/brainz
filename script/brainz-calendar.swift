// Brainz calendar helper. Prints the next N days of events as JSON so the
// Calendar tab can render them. Plain EventKit reading:
// a fresh EKEventStore per read (cached stores go stale), timed and all-day
// events, every account macOS Calendar knows about.
import EventKit
import Foundation

func emit(_ status: String, _ events: [[String: Any]]) -> Never {
    let payload: [String: Any] = ["status": status, "events": events]
    let data = try! JSONSerialization.data(withJSONObject: payload)
    FileHandle.standardOutput.write(data)
    exit(0)
}

let days = Int(CommandLine.arguments.dropFirst().first ?? "7") ?? 7

if EKEventStore.authorizationStatus(for: .event) == .notDetermined {
    let gate = DispatchSemaphore(value: 0)
    EKEventStore().requestFullAccessToEvents { _, _ in gate.signal() }
    gate.wait()
}
switch EKEventStore.authorizationStatus(for: .event) {
case .fullAccess: break
case .denied: emit("denied", [])
case .restricted: emit("restricted", [])
case .writeOnly: emit("write-only", [])
case .notDetermined: emit("not-determined", [])
@unknown default: emit("unknown", [])
}

let calendar = Calendar.current
let start = calendar.startOfDay(for: Date())
let end = calendar.date(byAdding: .day, value: days, to: start)!
let store = EKEventStore()
let events = store
    .events(matching: store.predicateForEvents(withStart: start, end: end, calendars: nil))
    .sorted { $0.startDate < $1.startDate }

let iso = ISO8601DateFormatter()
iso.formatOptions = [.withInternetDateTime]

func hex(_ color: CGColor?) -> String? {
    guard let color,
          let rgb = color.converted(to: CGColorSpaceCreateDeviceRGB(), intent: .defaultIntent, options: nil),
          let parts = rgb.components, parts.count >= 3 else { return nil }
    return String(format: "#%02x%02x%02x", Int(parts[0] * 255), Int(parts[1] * 255), Int(parts[2] * 255))
}

let list: [[String: Any]] = events.map { event in
    var item: [String: Any] = [
        "id": event.eventIdentifier ?? "",
        "title": event.title ?? "Untitled",
        "start": iso.string(from: event.startDate),
        "end": iso.string(from: event.endDate),
        "all_day": event.isAllDay,
        "calendar": event.calendar.title,
        "attendees": event.attendees?.count ?? 0,
    ]
    if let location = event.location, !location.isEmpty { item["location"] = location }
    if let notes = event.notes, !notes.isEmpty { item["notes"] = String(notes.prefix(2000)) }
    let names = (event.attendees ?? []).compactMap { $0.name }.filter { !$0.isEmpty }
    if !names.isEmpty { item["attendee_names"] = Array(names.prefix(30)) }
    if let organizer = event.organizer?.name, !organizer.isEmpty { item["organizer"] = organizer }
    // Brainz matches attendees to the brain by email domain, so the mailto
    // addresses travel too (same cap as the names).
    let emails = (event.attendees ?? []).compactMap { participant -> String? in
        guard let url = participant.url as URL?, url.scheme?.lowercased() == "mailto" else { return nil }
        let address = url.absoluteString.dropFirst("mailto:".count)
        return address.isEmpty ? nil : String(address).lowercased()
    }
    if !emails.isEmpty { item["attendee_emails"] = Array(emails.prefix(30)) }
    if let color = hex(event.calendar.cgColor) { item["color"] = color }
    if let url = event.url?.absoluteString { item["url"] = url }
    return item
}
emit("ok", list)
