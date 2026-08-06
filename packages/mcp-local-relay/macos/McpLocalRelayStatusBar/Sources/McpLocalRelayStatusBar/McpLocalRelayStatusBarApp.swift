import AppKit
import SwiftUI

private var retainedActionPanels: [NSPanel] = []
private let actionPanelDelegate = ActionPanelDelegate()

@main
struct McpLocalRelayStatusBarApp: App {
    @StateObject private var model = RelayStatusModel()

    var body: some Scene {
        MenuBarExtra {
            RelayMenu(model: model)
                .task {
                    await model.refresh()
                }
        } label: {
            Label(model.menuTitle, systemImage: model.menuIcon)
        }
        .menuBarExtraStyle(.menu)
    }
}

@MainActor
final class RelayStatusModel: ObservableObject {
    @Published var status: RelayStatus?
    @Published var menuStatus: RelayMenuStatus?
    @Published var corpusStatus: CorpusCodexStatus?
    @Published var errorMessage = ""
    @Published var corpusErrorMessage = ""
    @Published var isLoading = false

    private let baseURL = URL(string: "http://127.0.0.1:3764")!
    private let corpusBaseURL = URL(string: "http://127.0.0.1:3768")!
    private let decoder = JSONDecoder()
    private var pollingTask: Task<Void, Never>?
    private var willSleepObserver: NSObjectProtocol?
    private var didWakeObserver: NSObjectProtocol?

    init() {
        willSleepObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.willSleepNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { await self?.autoPauseForSleep() }
        }
        didWakeObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { await self?.autoResumeAfterWake() }
        }
        pollingTask = Task { [weak self] in
            await self?.refresh()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(10))
                await self?.refresh()
            }
        }
    }

    deinit {
        pollingTask?.cancel()
        if let willSleepObserver {
            NSWorkspace.shared.notificationCenter.removeObserver(willSleepObserver)
        }
        if let didWakeObserver {
            NSWorkspace.shared.notificationCenter.removeObserver(didWakeObserver)
        }
    }

    var menuTitle: String {
        if isLoading { return "Relay" }
        if let status, status.ok { return "Relay" }
        return "Relay"
    }

    var menuIcon: String {
        if status?.ok == true { return "bolt.horizontal.circle.fill" }
        if status != nil { return "exclamationmark.triangle.fill" }
        return "bolt.horizontal.circle"
    }

    var corpusMenuTitle: String {
        guard let corpusStatus else { return "Corpus Codex · Offline" }
        var parts = ["Corpus Codex", corpusStatus.state.capitalized, "\(corpusStatus.done) done"]
        if corpusStatus.failed > 0 { parts.append("\(corpusStatus.failed) failed") }
        if corpusStatus.needsHuman > 0 { parts.append("\(corpusStatus.needsHuman) human") }
        return parts.joined(separator: " · ")
    }

    func refresh() async {
        isLoading = true
        defer { isLoading = false }

        do {
            async let statusResult = fetchStatus()
            async let menuResult = fetchMenu()
            status = try await statusResult
            menuStatus = try await menuResult
            errorMessage = ""
        } catch {
            status = nil
            menuStatus = nil
            errorMessage = error.localizedDescription
        }
    }

    private func fetchStatus() async throws -> RelayStatus {
        let (data, response) = try await URLSession.shared.data(from: baseURL.appending(path: "status"))
        try check(response)
        return try decoder.decode(RelayStatus.self, from: data)
    }

    private func fetchMenu() async throws -> RelayMenuStatus {
        let (data, response) = try await URLSession.shared.data(from: baseURL.appending(path: "menu"))
        try check(response)
        return try decoder.decode(RelayMenuStatus.self, from: data)
    }

    func refreshCorpus() async {
        do {
            let (data, response) = try await URLSession.shared.data(from: corpusBaseURL.appending(path: "status"))
            try check(response)
            corpusStatus = try decoder.decode(CorpusCodexStatus.self, from: data)
            corpusErrorMessage = ""
        } catch {
            corpusStatus = nil
            corpusErrorMessage = error.localizedDescription
        }
    }

    func refreshServer(id: String) async {
        await refresh()
    }

    func refreshAllServers() async {
        await refresh()
    }

    func runMenuAction(serverId: String, actionId: String, confirm: Bool, args: [String: Any] = [:]) async -> MenuActionResult? {
        var request = URLRequest(url: baseURL.appending(path: "servers/\(serverId)/menu/actions/\(actionId)"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        request.httpBody = try? JSONSerialization.data(withJSONObject: ["confirm": confirm, "args": args])
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            try check(response)
            errorMessage = ""
            await refresh()
            return menuActionResult(from: data)
        } catch {
            errorMessage = error.localizedDescription
            await refresh()
            return MenuActionResult(summary: "Action failed", rows: [ActionResultRow(label: "Error", value: error.localizedDescription)], raw: nil)
        }
    }

    func restartRelay() async {
        await post(path: "restart")
        try? await Task.sleep(for: .seconds(1))
        await refresh()
    }

    func corpusStart() async {
        await postCorpus(path: "start", body: [:])
        await refreshCorpus()
    }

    func corpusPause() async {
        await postCorpus(path: "pause", body: [:])
        await refreshCorpus()
    }

    func corpusResume() async {
        await postCorpus(path: "resume", body: [:])
        await refreshCorpus()
    }

    func corpusDrain() async {
        await postCorpus(path: "drain", body: [:])
        await refreshCorpus()
    }

    func corpusStopAfterCurrent() async {
        await postCorpus(path: "stop-after-current", body: [:])
        await refreshCorpus()
    }

    func corpusPushCompleted() async {
        await postCorpus(path: "push-completed", body: [:])
        await refreshCorpus()
    }

    func copyClientConfig() async {
        do {
            let (data, response) = try await URLSession.shared.data(from: baseURL.appending(path: "client-config"))
            try check(response)
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(String(decoding: data, as: UTF8.self), forType: .string)
            errorMessage = ""
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func openLogsFolder() {
        let url = FileManager.default
            .homeDirectoryForCurrentUser
            .appending(path: ".local/state/mcp-local-relay", directoryHint: .isDirectory)
        NSWorkspace.shared.open(url)
    }

    func openCorpusLedger() {
        NSWorkspace.shared.open(URL(filePath: "/Users/astemarie/code/insurance-corpus/eval/batch/codex-ledger.jsonl"))
    }

    func openCorpusFailures() {
        NSWorkspace.shared.open(URL(string: "http://127.0.0.1:3768/failures")!)
    }

    private func autoPauseForSleep() async {
        guard corpusStatus?.state == "running" else { return }
        await postCorpus(path: "pause", body: ["pausedBy": "status-bar", "reason": "system_sleep"])
        await refreshCorpus()
    }

    private func autoResumeAfterWake() async {
        guard corpusStatus?.pausedBy == "status-bar" else {
            await refreshCorpus()
            return
        }
        await postCorpus(path: "resume", body: ["autoOnly": true])
        await refreshCorpus()
    }

    private func post(path: String) async {
        var request = URLRequest(url: baseURL.appending(path: path))
        request.httpMethod = "POST"
        do {
            let (_, response) = try await URLSession.shared.data(for: request)
            try check(response)
            errorMessage = ""
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func postCorpus(path: String, body: [String: Any]) async {
        var request = URLRequest(url: corpusBaseURL.appending(path: path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        request.httpBody = try? JSONSerialization.data(withJSONObject: body)
        do {
            let (_, response) = try await URLSession.shared.data(for: request)
            try check(response)
            corpusErrorMessage = ""
        } catch {
            corpusErrorMessage = error.localizedDescription
        }
    }

    private func check(_ response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw RelayError.requestFailed
        }
    }
}

struct RelayMenu: View {
    @ObservedObject var model: RelayStatusModel

    var body: some View {
        if let status = model.status {
            Section {
                Label(status.compactSummary, systemImage: status.ok ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                Label("Uptime \(formatUptime(status.uptimeMs)) · Sessions \(status.sessions)", systemImage: "clock")
            }

            if status.servers.isEmpty {
                Section("Servers") {
                    Text("No upstream MCP servers")
                }
            } else {
                ForEach(status.serverGroups) { group in
                    Section(group.name) {
                        ForEach(group.servers) { server in
                            ServerMenu(model: model, server: server, menu: model.menuStatus?.server(id: server.id))
                        }
                    }
                }
            }
        } else {
            Label("Relay offline", systemImage: "wifi.exclamationmark")
        }

        if !model.errorMessage.isEmpty {
            Section {
                Label(model.errorMessage, systemImage: "exclamationmark.triangle")
            }
        }

        Section {
            Button {
                Task { await model.refresh() }
            } label: {
                Label("Refresh Status", systemImage: "arrow.clockwise")
            }
            Button {
                Task { await model.refreshAllServers() }
            } label: {
                Label("Refresh All MCPs", systemImage: "arrow.triangle.2.circlepath")
            }
            Button {
                Task { await model.copyClientConfig() }
            } label: {
                Label("Copy Client Config", systemImage: "doc.on.doc")
            }
            Button {
                model.openLogsFolder()
            } label: {
                Label("Open Logs Folder", systemImage: "folder")
            }
            Button {
                Task { await model.restartRelay() }
            } label: {
                Label("Restart Relay", systemImage: "power")
            }
            Divider()
            Button("Quit") {
                NSApplication.shared.terminate(nil)
            }
        }
    }
}

struct ServerMenu: View {
    @ObservedObject var model: RelayStatusModel
    let server: RelayServerStatus
    let menu: RelayMenuServer?

    var body: some View {
        Menu {
            if let menu {
                Label(menu.summary, systemImage: menu.systemImage)
                ForEach(menu.detail.prefix(8), id: \.self) { line in
                    Text(line)
                }
                Divider()
                if menu.actions.isEmpty {
                    Text("No quick actions")
                } else {
                    ForEach(menu.actions) { action in
                        Button {
                            if let view = action.view {
                                DispatchQueue.main.async {
                                    showRelayViewWindow(title: action.label, view: view)
                                }
                                return
                            }
                            if let url = action.url, action.tool == nil, action.input == nil, let parsed = URL(string: url) {
                                NSWorkspace.shared.open(parsed)
                                return
                            }
                            let args: [String: Any]
                            if let input = action.input {
                                guard let prompted = promptForActionInputWindow(input) else { return }
                                args = prompted
                            } else {
                                args = [:]
                            }
                            if action.confirm && !confirmMenuAction(action) {
                                return
                            }
                            Task {
                                if let result = await model.runMenuAction(
                                    serverId: server.id,
                                    actionId: action.id,
                                    confirm: action.confirm,
                                    args: args
                                ) {
                                    await MainActor.run {
                                        showActionResultWindow(title: action.label, result: result)
                                    }
                                }
                            }
                        } label: {
                            Label(action.label, systemImage: action.systemImage ?? (action.view == nil ? "bolt" : "tablecells"))
                        }
                    }
                }
            } else {
                if !server.detailText.isEmpty {
                    Text(server.detailText)
                }
                StatusLine(title: "ID", value: server.id, systemImage: "number")
                StatusLine(title: "Status", value: server.statusText, systemImage: server.statusIcon)
                StatusLine(title: "Tools", value: String(server.cachedTools), systemImage: "wrench.and.screwdriver")
                StatusLine(title: "Last refresh", value: formatTime(server.cachedAt), systemImage: "clock.arrow.circlepath")
                if !server.lastRefreshError.isEmpty {
                    Text(server.lastRefreshError)
                }
            }
            Divider()
            Button {
                Task { await model.refreshServer(id: server.id) }
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise")
            }
        } label: {
            Label(menu?.compactTitle ?? server.compactTitle, systemImage: menu?.systemImage ?? server.statusIcon)
        }
    }
}

func confirmMenuAction(_ action: RelayMenuAction) -> Bool {
    let alert = NSAlert()
    alert.messageText = action.label
    alert.informativeText = "Run this action?"
    alert.alertStyle = .warning
    alert.addButton(withTitle: "Run")
    alert.addButton(withTitle: "Cancel")
    return alert.runModal() == .alertFirstButtonReturn
}

func promptForActionInputWindow(_ input: RelayActionInput) -> [String: Any]? {
    let controller = ActionInputPanelController(input: input)
    return controller.run()
}

func showRelayViewWindow(title: String, view: RelayActionView) {
    let panel = floatingPanel(title: title, width: 620, height: 430)
    panel.contentView = NSHostingView(rootView: RelayActionViewWindow(view: view))
    retainActionPanel(panel)
    panel.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
}

func showActionResultWindow(title: String, result: MenuActionResult) {
    let panel = floatingPanel(title: title, width: 620, height: 430)
    panel.contentView = NSHostingView(rootView: ActionResultWindow(title: title, result: result))
    retainActionPanel(panel)
    panel.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
}

func retainActionPanel(_ panel: NSPanel) {
    panel.isReleasedWhenClosed = false
    panel.delegate = actionPanelDelegate
    retainedActionPanels.append(panel)
}

final class ActionPanelDelegate: NSObject, NSWindowDelegate {
    func windowWillClose(_ notification: Notification) {
        guard let panel = notification.object as? NSPanel else { return }
        retainedActionPanels.removeAll { $0 === panel }
    }
}

func floatingPanel(title: String, width: CGFloat, height: CGFloat) -> NSPanel {
    let panel = NSPanel(
        contentRect: NSRect(x: 0, y: 0, width: width, height: height),
        styleMask: [.titled, .closable, .resizable, .utilityWindow],
        backing: .buffered,
        defer: false
    )
    panel.title = title
    panel.isFloatingPanel = true
    panel.hidesOnDeactivate = false
    panel.center()
    return panel
}

func menuActionResult(from data: Data) -> MenuActionResult? {
    guard !data.isEmpty else { return nil }
    let object = (try? JSONSerialization.jsonObject(with: data)) ?? String(data: data, encoding: .utf8) ?? ""
    let unwrapped = unwrapMcpText(object) ?? object
    let parsed = parseJsonString(unwrapped) ?? unwrapped
    return renderableActionResult(parsed)
}

func unwrapMcpText(_ object: Any) -> Any? {
    guard let dict = object as? [String: Any],
          let content = dict["content"] as? [[String: Any]],
          let text = content.first?["text"] as? String else {
        return nil
    }
    return text
}

func parseJsonString(_ object: Any) -> Any? {
    guard let text = object as? String,
          let data = text.data(using: .utf8) else {
        return nil
    }
    return try? JSONSerialization.jsonObject(with: data)
}

func renderableActionResult(_ object: Any) -> MenuActionResult? {
    if let dict = object as? [String: Any] {
        let status = dict["status"] as? [String: Any]
        let state = stringValue(status?["state"] ?? dict["state"])
        let started = boolDisplay(dict["started"])
        let summary: String
        if started == "true" && !state.isEmpty {
            summary = state == "paused" ? "Started, currently paused" : "Started, \(state)"
        } else if started == "true" {
            summary = "Started"
        } else if !state.isEmpty {
            summary = state.capitalized
        } else {
            summary = "Action completed"
        }

        let rows = actionRows(from: dict, status: status)
        return rows.isEmpty ? nil : MenuActionResult(summary: summary, rows: rows, raw: nil)
    }
    if let text = object as? String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : MenuActionResult(summary: "Action completed", rows: [ActionResultRow(label: "Result", value: trimmed)], raw: nil)
    }
    return nil
}

func actionRows(from dict: [String: Any], status: [String: Any]?) -> [ActionResultRow] {
    let keys: [(String, Any?)] = [
        ("Started", dict["started"]),
        ("State", status?["state"] ?? dict["state"]),
        ("PID", dict["pid"] ?? status?["pid"]),
        ("PID alive", status?["pidAlive"]),
        ("Workers", status?["workers"] ?? dict["workers"]),
        ("Active", status?["active"]),
        ("Dispatched", status?["dispatched"]),
        ("Done", status?["done"]),
        ("Failed", status?["failed"]),
        ("Needs human", status?["needsHuman"]),
        ("Pending", status?["pending"]),
        ("Paused by", status?["pausedBy"]),
        ("Last heartbeat", status?["lastHeartbeatAt"]),
    ]
    return keys.compactMap { label, value in
        let text = stringValue(value)
        return text.isEmpty || text == "null" ? nil : ActionResultRow(label: label, value: text)
    }
}

func stringValue(_ value: Any?) -> String {
    guard let value else { return "" }
    if value is NSNull { return "" }
    if let value = value as? String { return value }
    if let value = value as? Bool { return value ? "true" : "false" }
    if let value = value as? NSNumber { return value.stringValue }
    return String(describing: value)
}

func boolDisplay(_ value: Any?) -> String {
    stringValue(value).lowercased()
}

final class ActionInputPanelController {
    private let input: RelayActionInput
    private var result: [String: Any]?
    private var panel: NSPanel?

    init(input: RelayActionInput) {
        self.input = input
    }

    func run() -> [String: Any]? {
        let height = CGFloat(min(620, max(300, 132 + input.fields.count * 72)))
        let panel = floatingPanel(title: input.title, width: 520, height: height)
        self.panel = panel
        panel.contentView = NSHostingView(rootView: ActionInputForm(
            input: input,
            onCancel: { [weak self] in self?.finish(nil) },
            onSubmit: { [weak self] args in self?.finish(args) }
        ))
        panel.makeKeyAndOrderFront(nil as Any?)
        NSApp.activate(ignoringOtherApps: true)
        NSApp.runModal(for: panel)
        return result
    }

    private func finish(_ value: [String: Any]?) {
        result = value
        if let panel {
            NSApp.stopModal()
            panel.close()
        }
    }
}

struct ActionInputForm: View {
    let input: RelayActionInput
    let onCancel: () -> Void
    let onSubmit: ([String: Any]) -> Void

    @State private var values: [String: String]
    @State private var validationMessage = ""

    init(input: RelayActionInput, onCancel: @escaping () -> Void, onSubmit: @escaping ([String: Any]) -> Void) {
        self.input = input
        self.onCancel = onCancel
        self.onSubmit = onSubmit
        _values = State(initialValue: Dictionary(uniqueKeysWithValues: input.fields.map { ($0.id, $0.defaultValue) }))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(input.title)
                .font(.headline)

            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    ForEach(input.fields) { field in
                        VStack(alignment: .leading, spacing: 5) {
                            HStack(spacing: 3) {
                                Text(field.label)
                                    .font(.subheadline)
                                    .fontWeight(.medium)
                                if field.required {
                                    Text("*")
                                        .foregroundStyle(.red)
                                }
                            }
                            if field.type == "boolean" {
                                Toggle("Enabled", isOn: Binding(
                                    get: { boolValue(values[field.id] ?? field.defaultValue) },
                                    set: { values[field.id] = $0 ? "true" : "false" }
                                ))
                            } else if field.multiline {
                                TextEditor(text: binding(for: field))
                                    .font(.body)
                                    .frame(minHeight: 84)
                                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.secondary.opacity(0.25)))
                            } else {
                                TextField(field.placeholder ?? "", text: binding(for: field))
                                    .textFieldStyle(.roundedBorder)
                            }
                        }
                    }
                }
                .padding(.vertical, 2)
            }

            if !validationMessage.isEmpty {
                Label(validationMessage, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
                    .font(.caption)
            }

            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button(input.submitLabel) {
                    submit()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(18)
        .frame(minWidth: 460, minHeight: 260)
    }

    private func binding(for field: RelayActionInputField) -> Binding<String> {
        Binding(
            get: { values[field.id] ?? field.defaultValue },
            set: { values[field.id] = $0 }
        )
    }

    private func submit() {
        var args: [String: Any] = [:]
        for field in input.fields {
            let rawValue = (values[field.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if field.required && rawValue.isEmpty {
                validationMessage = "\(field.label) is required."
                return
            }
            guard !rawValue.isEmpty else { continue }
            switch field.type {
            case "number":
                if let intValue = Int(rawValue) {
                    args[field.id] = intValue
                } else if let doubleValue = Double(rawValue) {
                    args[field.id] = doubleValue
                } else {
                    validationMessage = "\(field.label) must be a number."
                    return
                }
            case "boolean":
                args[field.id] = boolValue(rawValue)
            default:
                args[field.id] = rawValue
            }
        }
        onSubmit(args)
    }
}

struct RelayActionViewWindow: View {
    let view: RelayActionView

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text(view.title)
                        .font(.headline)
                    if !view.summary.isEmpty {
                        Text(view.summary)
                            .foregroundStyle(.secondary)
                    }
                }
                Spacer()
                if let refreshSeconds = view.refreshSeconds {
                    Label("\(refreshSeconds)s", systemImage: "arrow.clockwise")
                        .foregroundStyle(.secondary)
                }
            }

            Divider()

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    if view.rows.isEmpty {
                        Text("No rows")
                            .foregroundStyle(.secondary)
                    } else {
                        ForEach(Array(view.rows.enumerated()), id: \.offset) { _, row in
                            RelayActionRow(row: row, columns: view.columns)
                        }
                    }
                }
                .padding(.vertical, 2)
            }

            if !view.footerActions.isEmpty {
                Divider()
                HStack {
                    Spacer()
                    ForEach(view.footerActions) { action in
                        Button {
                            if let parsed = URL(string: action.url) {
                                NSWorkspace.shared.open(parsed)
                            }
                        } label: {
                            Label(action.label, systemImage: action.systemImage ?? "arrow.up.forward.app")
                        }
                    }
                }
            }
        }
        .padding(16)
        .frame(minWidth: 520, minHeight: 320)
    }
}

struct RelayActionRow: View {
    let row: RelayViewRow
    let columns: [RelayViewColumn]

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: row.statusIcon)
                .foregroundStyle(row.statusColor)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 3) {
                Text(row.primary(columns: columns))
                    .fontWeight(.medium)
                    .lineLimit(1)
                let secondary = row.secondary(columns: columns)
                if !secondary.isEmpty {
                    Text(secondary)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 8)
            if let link = row.linkUrl {
                Button {
                    NSWorkspace.shared.open(link)
                } label: {
                    Label(row.linkLabel, systemImage: "arrow.up.forward.app")
                        .labelStyle(.titleAndIcon)
                }
                .buttonStyle(.borderless)
            }
        }
        .padding(8)
        .background(Color.secondary.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 7))
    }
}

struct ActionResultWindow: View {
    let title: String
    let result: MenuActionResult

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.headline)
                Text(result.summary)
                    .foregroundStyle(.secondary)
            }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    ForEach(result.rows) { row in
                        HStack(alignment: .top) {
                            Text(row.label)
                                .foregroundStyle(.secondary)
                                .frame(width: 120, alignment: .leading)
                            Text(row.value)
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .padding(8)
                        .background(Color.secondary.opacity(0.08))
                        .clipShape(RoundedRectangle(cornerRadius: 7))
                    }
                }
            }
        }
        .padding(16)
        .frame(minWidth: 520, minHeight: 320)
    }
}

struct MenuActionResult {
    let summary: String
    let rows: [ActionResultRow]
    let raw: String?
}

struct ActionResultRow: Identifiable {
    let id = UUID()
    let label: String
    let value: String
}

func boolValue(_ value: String) -> Bool {
    ["1", "true", "yes", "y", "on"].contains(value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased())
}

struct CompactRelayView: View {
    let view: RelayActionView

    var body: some View {
        if !view.summary.isEmpty {
            Label(view.summary, systemImage: "chart.bar.doc.horizontal")
        } else {
            Label(view.title, systemImage: "tablecells")
        }
        if let refreshSeconds = view.refreshSeconds {
            Text("Refreshes every \(refreshSeconds)s")
        }
        Divider()
        if view.rows.isEmpty {
            Text("No rows")
        } else {
            ForEach(Array(view.rows.prefix(8).enumerated()), id: \.offset) { _, row in
                Label(row.display(columns: view.columns), systemImage: row.statusIcon)
            }
        }
        if !view.footerActions.isEmpty {
            Divider()
            ForEach(view.footerActions) { action in
                Button {
                    if let parsed = URL(string: action.url) {
                        NSWorkspace.shared.open(parsed)
                    }
                } label: {
                    Label(action.label, systemImage: action.systemImage ?? "arrow.up.forward.app")
                }
            }
        }
    }
}

struct StatusLine: View {
    let title: String
    let value: String
    let systemImage: String

    var body: some View {
        Label("\(title): \(value)", systemImage: systemImage)
    }
}

struct RelayStatus: Decodable {
    let ok: Bool
    let uptimeMs: Int
    let sessions: Int
    let servers: [RelayServerStatus]

    var healthyServerCount: Int {
        servers.filter(\.isHealthy).count
    }

    var totalTools: Int {
        servers.reduce(0) { $0 + $1.cachedTools }
    }

    var compactSummary: String {
        let state = ok ? "Running" : "Needs attention"
        return "\(state): \(healthyServerCount)/\(servers.count) MCPs · \(totalTools) tools"
    }

    var serverGroups: [RelayServerGroup] {
        let sortedServers = servers.sorted {
            $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
        }
        let grouped = Dictionary(grouping: sortedServers) { $0.categoryText }
        return grouped
            .map { RelayServerGroup(name: $0.key, servers: $0.value) }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }
}

struct RelayMenuStatus: Decodable {
    let ok: Bool
    let servers: [RelayMenuServer]

    func server(id: String) -> RelayMenuServer? {
        servers.first { $0.id == id }
    }
}

struct RelayMenuServer: Decodable, Identifiable {
    let id: String
    let title: String
    let summary: String
    let state: String
    let detail: [String]
    let actions: [RelayMenuAction]
    let cachedAt: Int?
    let lastError: String?

    var compactTitle: String {
        "\(title) · \(summary)"
    }

    var systemImage: String {
        switch state.lowercased() {
        case "ready", "complete": return "checkmark.circle"
        case "running", "syncing": return "arrow.triangle.2.circlepath"
        case "paused": return "pause.circle"
        case "stale", "attention", "error": return "exclamationmark.triangle"
        case "empty": return "tray"
        default: return "circle"
        }
    }
}

struct RelayMenuAction: Decodable, Identifiable {
    let id: String
    let label: String
    let systemImage: String?
    let confirm: Bool
    let tool: String?
    let url: String?
    let view: RelayActionView?
    let input: RelayActionInput?

    private enum CodingKeys: String, CodingKey {
        case id, label, systemImage, confirm, tool, url, view, input
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        label = try c.decode(String.self, forKey: .label)
        systemImage = try c.decodeIfPresent(String.self, forKey: .systemImage)
        confirm = try c.decodeIfPresent(Bool.self, forKey: .confirm) ?? false
        tool = try c.decodeIfPresent(String.self, forKey: .tool)
        url = try c.decodeIfPresent(String.self, forKey: .url)
        view = try c.decodeIfPresent(RelayActionView.self, forKey: .view)
        input = try c.decodeIfPresent(RelayActionInput.self, forKey: .input)
    }
}

struct RelayActionInput: Decodable {
    let title: String
    let submitLabel: String
    let fields: [RelayActionInputField]

    private enum CodingKeys: String, CodingKey {
        case title, submitLabel, fields
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? "Action Input"
        submitLabel = try c.decodeIfPresent(String.self, forKey: .submitLabel) ?? "Run"
        fields = try c.decodeIfPresent([RelayActionInputField].self, forKey: .fields) ?? []
    }
}

struct RelayActionInputField: Decodable, Identifiable {
    let id: String
    let label: String
    let type: String
    let placeholder: String?
    let defaultRaw: RelayInputValue?
    let required: Bool
    let multiline: Bool

    private enum CodingKeys: String, CodingKey {
        case id, label, type, placeholder, defaultRaw = "default", required, multiline
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        label = try c.decode(String.self, forKey: .label)
        type = try c.decodeIfPresent(String.self, forKey: .type) ?? "string"
        placeholder = try c.decodeIfPresent(String.self, forKey: .placeholder)
        defaultRaw = try c.decodeIfPresent(RelayInputValue.self, forKey: .defaultRaw)
        required = try c.decodeIfPresent(Bool.self, forKey: .required) ?? false
        multiline = try c.decodeIfPresent(Bool.self, forKey: .multiline) ?? false
    }

    var defaultValue: String {
        defaultRaw?.stringValue ?? ""
    }
}

enum RelayInputValue: Decodable {
    case string(String)
    case number(Double)
    case bool(Bool)

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if let value = try? c.decode(String.self) {
            self = .string(value)
        } else if let value = try? c.decode(Double.self) {
            self = .number(value)
        } else {
            self = .bool(try c.decode(Bool.self))
        }
    }

    var stringValue: String {
        switch self {
        case .string(let value): return value
        case .number(let value):
            return value.rounded() == value ? String(Int(value)) : String(value)
        case .bool(let value): return value ? "true" : "false"
        }
    }
}

struct RelayActionView: Decodable {
    let type: String
    let title: String
    let summary: String
    let refreshSeconds: Int?
    let density: String?
    let columns: [RelayViewColumn]
    let rows: [RelayViewRow]
    let footerActions: [RelayViewFooterAction]

    private enum CodingKeys: String, CodingKey {
        case type, title, summary, refreshSeconds, density, columns, rows, footerActions
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        type = try c.decode(String.self, forKey: .type)
        title = try c.decode(String.self, forKey: .title)
        summary = try c.decodeIfPresent(String.self, forKey: .summary) ?? ""
        refreshSeconds = try c.decodeIfPresent(Int.self, forKey: .refreshSeconds)
        density = try c.decodeIfPresent(String.self, forKey: .density)
        columns = try c.decodeIfPresent([RelayViewColumn].self, forKey: .columns) ?? []
        rows = try c.decodeIfPresent([RelayViewRow].self, forKey: .rows) ?? []
        footerActions = try c.decodeIfPresent([RelayViewFooterAction].self, forKey: .footerActions) ?? []
    }
}

struct RelayViewColumn: Decodable {
    let id: String
    let label: String
    let kind: String?
}

struct RelayViewRow: Decodable {
    let values: [String: String]

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: DynamicCodingKey.self)
        var next: [String: String] = [:]
        for key in c.allKeys {
            if let string = try? c.decode(String.self, forKey: key) {
                next[key.stringValue] = string
            } else if let int = try? c.decode(Int.self, forKey: key) {
                next[key.stringValue] = String(int)
            } else if let double = try? c.decode(Double.self, forKey: key) {
                next[key.stringValue] = String(double)
            } else if let bool = try? c.decode(Bool.self, forKey: key) {
                next[key.stringValue] = bool ? "true" : "false"
            }
        }
        values = next
    }

    var statusIcon: String {
        switch values["status"]?.lowercased() {
        case "success": return "checkmark.circle.fill"
        case "running": return "arrow.triangle.2.circlepath.circle.fill"
        case "warning": return "exclamationmark.triangle.fill"
        case "error": return "xmark.octagon.fill"
        case "paused": return "pause.circle.fill"
        default: return "circle.fill"
        }
    }

    var statusColor: Color {
        switch values["status"]?.lowercased() {
        case "success": return .green
        case "running": return .blue
        case "warning": return .yellow
        case "error": return .red
        case "paused": return .orange
        default: return .secondary
        }
    }

    var linkUrl: URL? {
        guard let raw = values["linkUrl"] ?? values["url"], !raw.isEmpty else { return nil }
        return URL(string: raw)
    }

    var linkLabel: String {
        let label = values["linkLabel"] ?? "Open"
        return label.isEmpty ? "Open" : label
    }

    func primary(columns: [RelayViewColumn]) -> String {
        let visibleColumns = columns.filter { $0.kind != "status" }
        let preferred = ["plan", "item", "documentId", "slug", "title", "name"]
        if let key = preferred.first(where: { !(values[$0] ?? "").isEmpty }) {
            return values[key] ?? ""
        }
        if let first = visibleColumns.first(where: { !(values[$0.id] ?? "").isEmpty }) {
            return values[first.id] ?? ""
        }
        return display(columns: columns)
    }

    func secondary(columns: [RelayViewColumn]) -> String {
        let primaryValue = primary(columns: columns)
        let visibleColumns = columns.filter { $0.kind != "status" }
        let ordered = visibleColumns.isEmpty ? values.keys.sorted() : visibleColumns.map(\.id)
        return ordered
            .compactMap { values[$0] }
            .filter { !$0.isEmpty && $0 != primaryValue }
            .filter { $0 != values["linkLabel"] && $0 != values["linkUrl"] && $0 != values["url"] }
            .joined(separator: " · ")
    }

    func display(columns: [RelayViewColumn]) -> String {
        let visibleColumns = columns.filter { $0.kind != "status" }
        let ordered = visibleColumns.isEmpty ? values.keys.sorted() : visibleColumns.map(\.id)
        return ordered
            .compactMap { values[$0] }
            .filter { !$0.isEmpty }
            .filter { $0 != values["linkLabel"] && $0 != values["linkUrl"] && $0 != values["url"] }
            .joined(separator: " · ")
    }
}

struct RelayViewFooterAction: Decodable, Identifiable {
    let id: String
    let label: String
    let systemImage: String?
    let url: String
}

struct DynamicCodingKey: CodingKey {
    let stringValue: String
    let intValue: Int?

    init?(stringValue: String) {
        self.stringValue = stringValue
        intValue = nil
    }

    init?(intValue: Int) {
        stringValue = String(intValue)
        self.intValue = intValue
    }
}

struct RelayServerGroup: Identifiable {
    let name: String
    let servers: [RelayServerStatus]

    var id: String { name }
}

struct RelayServerStatus: Decodable, Identifiable {
    let id: String
    let name: String
    let description: String?
    let category: String?
    let enabled: Bool
    let connected: Bool
    let cachedTools: Int
    let cachedAt: Int
    let lastRefreshError: String

    var isHealthy: Bool {
        enabled && lastRefreshError.isEmpty
    }

    var statusText: String {
        if !enabled { return "Disabled" }
        if !lastRefreshError.isEmpty { return "Error" }
        if connected { return "Connected" }
        return "Ready"
    }

    var statusIcon: String {
        if !enabled { return "pause.circle" }
        if !lastRefreshError.isEmpty { return "exclamationmark.triangle" }
        if connected { return "checkmark.circle" }
        return "circle"
    }

    var categoryText: String {
        let value = (category ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? "Other" : value
    }

    var detailText: String {
        (description ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var compactTitle: String {
        "\(name) · \(statusText) · \(cachedTools)"
    }
}

struct CorpusCodexStatus: Decodable {
    let ok: Bool
    let state: String
    let active: Int
    let workers: Int?
    let dispatched: Int
    let total: Int?
    let done: Int
    let failed: Int
    let needsHuman: Int
    let pending: Int?
    let lastCompleted: String?
    let lastStatus: String?
    let lastHeartbeatAt: String?
    let etaHours: Double?
    let pausedBy: String?

    var summary: String {
        var parts = ["\(state.capitalized)", "\(done) done"]
        if failed > 0 { parts.append("\(failed) failed") }
        if needsHuman > 0 { parts.append("\(needsHuman) needs human") }
        if let etaHours { parts.append("ETA \(formatEta(etaHours))") }
        return parts.joined(separator: " · ")
    }

    var detail: String {
        let workerText = workers.map { "\(active)/\($0)" } ?? "\(active)"
        let pendingText = pending.map { "\($0) pending" } ?? "pending unknown"
        let pauseText = pausedBy.map { " · paused by \($0)" } ?? ""
        return "Active \(workerText) · \(dispatched) dispatched · \(pendingText)\(pauseText)"
    }

    var systemImage: String {
        switch state.lowercased() {
        case "running": return "play.circle.fill"
        case "paused": return "pause.circle.fill"
        case "draining": return "arrow.down.circle.fill"
        case "stopping": return "stop.circle.fill"
        case "complete": return "checkmark.circle.fill"
        case "attention": return "exclamationmark.triangle.fill"
        default: return ok ? "doc.text.magnifyingglass" : "exclamationmark.triangle.fill"
        }
    }
}

private func formatEta(_ hours: Double) -> String {
    if hours >= 48 { return String(format: "%.1fd", hours / 24) }
    if hours >= 1 { return String(format: "%.1fh", hours) }
    return "\(Int(hours * 60))m"
}

enum RelayError: Error {
    case requestFailed
}

private func formatUptime(_ ms: Int) -> String {
    let seconds = max(0, ms / 1000)
    let hours = seconds / 3600
    let minutes = (seconds % 3600) / 60
    if hours > 0 { return "\(hours)h \(minutes)m" }
    return "\(minutes)m"
}

private func formatTime(_ epochMs: Int) -> String {
    guard epochMs > 0 else { return "Never" }
    let date = Date(timeIntervalSince1970: TimeInterval(epochMs) / 1000)
    return date.formatted(date: .omitted, time: .shortened)
}
