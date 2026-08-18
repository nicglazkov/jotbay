import SwiftUI

/// Read-only browser for the notes folder: one directory at a time, breadcrumb
/// back, markdown rendered on selection.
///
/// Reads the filesystem directly rather than shelling out to the CLI. This is
/// the one surface where the CLI has nothing to add: there is no git state
/// involved, and a preview that spawns a process per click feels like one.
struct FilesPane: View {
    @EnvironmentObject private var controller: JotbayController

    @State private var relPath = ""
    @State private var entries: [FileEntry] = []
    @State private var preview: Preview?
    @State private var problem: String?

    // Finding, and the two sheets that show a note over time. Sheets rather
    // than windows on purpose: settings is about the app and gets a window,
    // these are about what is in front of you and belong over it.
    @State private var query = ""
    @State private var hits: [Hit] = []
    @State private var searching = false
    @State private var historyFor: String?
    @State private var showDeleted = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            toolbar
            Divider()
            crumbs
            Divider()
            if !query.trimmingCharacters(in: .whitespaces).isEmpty {
                results
            } else if let preview {
                VStack(alignment: .leading, spacing: 0) {
                    HStack(spacing: 14) {
                        // The honest answer to "let me edit in the app": hand
                        // the note to whatever this person already writes in.
                        // The outward arrow says it leaves Jotbay, which the
                        // words alone did not.
                        Button {
                            controller.openInEditor(preview.rel)
                        } label: {
                            Label("Open in editor", systemImage: "arrow.up.forward.app")
                        }
                        // "History" sat one tab away from Activity, which is
                        // also a history, of something else entirely. Naming
                        // the thing it is a history *of* is the whole fix.
                        Button {
                            historyFor = preview.rel
                        } label: {
                            Label("Version history", systemImage: "clock.arrow.circlepath")
                        }
                        Spacer()
                    }
                    .buttonStyle(.borderless)
                    .labelStyle(.titleAndIcon)
                    .font(.system(size: 11))
                    .padding(.horizontal, 20)
                    .padding(.vertical, 7)
                    Divider()
                    PreviewView(preview: preview)
                }
            } else if let problem {
                EmptyPane(symbol: "exclamationmark.triangle", title: "Can't read that",
                          detail: problem)
            } else if entries.isEmpty {
                VStack(spacing: 0) {
                    EmptyPane(symbol: "tray", title: "Nothing here yet",
                              detail: "Files you put in this folder sync everywhere.")
                    // Reachable from an empty folder too. Somebody who has
                    // just deleted the last note is exactly who needs this.
                    if relPath.isEmpty { recentlyDeletedRow }
                }
            } else {
                listing
            }
        }
        .onAppear { load() }
        .sheet(item: Binding(
            get: { historyFor.map { HistoryTarget(rel: $0) } },
            set: { historyFor = $0?.rel }
        )) { target in
            HistorySheet(rel: target.rel).environmentObject(controller)
        }
        .sheet(isPresented: $showDeleted) {
            DeletedSheet().environmentObject(controller)
        }
        .sheet(isPresented: $controller.composing) {
            NewNoteSheet { load() }.environmentObject(controller)
        }
    }

    // MARK: - Finding, and the two ways back in time

    private var toolbar: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
            TextField("Search notes", text: $query)
                .textFieldStyle(.plain)
                .font(.system(size: 12))
                .onChange(of: query) { _, next in runSearch(next) }
            if !query.isEmpty {
                Button {
                    query = ""
                    hits = []
                } label: {
                    Image(systemName: "xmark.circle.fill").font(.system(size: 11))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.tertiary)
            }
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
    }

    private var results: some View {
        Group {
            if searching && hits.isEmpty {
                EmptyPane(symbol: "magnifyingglass", title: "Searching", detail: "")
            } else if hits.isEmpty {
                EmptyPane(symbol: "magnifyingglass", title: "No matches",
                          detail: "Nothing here contains that. Only notes that have synced at least once can be searched by content.")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(hits) { hit in
                            Button {
                                query = ""
                                hits = []
                                openFile(hit.path)
                            } label: {
                                HStack(alignment: .top, spacing: 10) {
                                    Image(systemName: hit.nameMatch ? "doc.text.fill" : "text.magnifyingglass")
                                        .font(.system(size: 11))
                                        .foregroundStyle(hit.nameMatch ? Color.accentColor : .secondary)
                                        .frame(width: 14)
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(hit.path).font(.system(size: 12))
                                        if let excerpt = hit.excerpt {
                                            Text(excerpt)
                                                .font(.system(size: 10, design: .monospaced))
                                                .foregroundStyle(.secondary)
                                                .lineLimit(2)
                                        }
                                    }
                                    Spacer()
                                }
                                .padding(.horizontal, 20)
                                .padding(.vertical, 7)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            Divider().padding(.leading, 20)
                        }
                    }
                }
            }
        }
    }

    /// Searched on a short delay rather than per keystroke: each search is a
    /// process, and typing "postgres" would otherwise start eight of them.
    private func runSearch(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else {
            hits = []
            searching = false
            return
        }
        searching = true
        Task {
            try? await Task.sleep(for: .milliseconds(180))
            guard query.trimmingCharacters(in: .whitespaces) == trimmed else { return }
            let found = await controller.search(trimmed)
            guard query.trimmingCharacters(in: .whitespaces) == trimmed else { return }
            hits = found
            searching = false
        }
    }

    // MARK: - Navigation

    private var crumbs: some View {
        HStack(spacing: 4) {
            Button("notes") { open("") }
                .buttonStyle(.link).font(.system(size: 12))
            let parts = crumbParts
            ForEach(Array(parts.enumerated()), id: \.offset) { index, part in
                Text("/").foregroundStyle(.tertiary).font(.system(size: 12))
                if index == parts.count - 1 {
                    Text(part.name).font(.system(size: 12)).foregroundStyle(.secondary)
                } else {
                    Button(part.name) { open(part.path) }
                        .buttonStyle(.link).font(.system(size: 12))
                }
            }
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
    }

    private var crumbParts: [(name: String, path: String)] {
        let shown = preview?.rel ?? relPath
        guard !shown.isEmpty else { return [] }
        var acc = ""
        return shown.split(separator: "/").map { piece in
            acc = acc.isEmpty ? String(piece) : "\(acc)/\(piece)"
            return (String(piece), acc)
        }
    }

    private var listing: some View {
        ScrollView {
            LazyVStack(spacing: 0) {
                ForEach(entries) { entry in
                    Button {
                        entry.isDir ? open(entry.rel) : show(entry)
                    } label: {
                        HStack(spacing: 10) {
                            Image(systemName: entry.isDir ? "folder.fill" : "doc.text")
                                .font(.system(size: 12))
                                .foregroundStyle(entry.isDir ? Color.accentColor : .secondary)
                                .frame(width: 16)
                            Text(entry.name)
                                .font(.system(size: 13))
                                .lineLimit(1)
                            Spacer()
                            Text(entry.detail)
                                .font(.system(size: 11))
                                .foregroundStyle(.tertiary)
                        }
                        .padding(.horizontal, 20)
                        .padding(.vertical, 7)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    Divider().padding(.leading, 20)
                }

                // A place, not a button.
                //
                // It was a toolbar button labelled "Deleted", which reads as a
                // filter and sat beside an action. Recovering a note is
                // somewhere you go, the way Photos and Mail treat it, so it
                // belongs at the end of the list looking like what it is.
                //
                // Only at the top level: shown inside design/ it would read as
                // that folder's deleted notes, and it is the whole vault's.
                if relPath.isEmpty {
                    recentlyDeletedRow
                }
            }
        }
    }

    private var recentlyDeletedRow: some View {
        VStack(spacing: 0) {
            // Set apart, because it is not one of the notes above it.
            Divider().padding(.vertical, 6).padding(.horizontal, 20)
            Button {
                showDeleted = true
            } label: {
                HStack(spacing: 10) {
                    Image(systemName: "trash")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .frame(width: 16)
                    Text("Recently Deleted")
                        .font(.system(size: 13))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.tertiary)
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 7)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }

    // MARK: - Data

    /// Jump straight to a note, which is what a search result is asking for.
    private func openFile(_ rel: String) {
        let parent = (rel as NSString).deletingLastPathComponent
        relPath = parent
        problem = nil
        load()
        if let match = entries.first(where: { $0.rel == rel && !$0.isDir }) {
            show(match)
        }
    }

    private func open(_ rel: String) {
        relPath = rel
        preview = nil
        problem = nil
        load()
    }

    private func load() {
        let dir = controller.dataDirectory.appendingPathComponent(relPath)
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(
            at: dir, includingPropertiesForKeys: [.isDirectoryKey, .fileSizeKey, .contentModificationDateKey],
            options: [.skipsHiddenFiles]
        ) else {
            entries = []
            return
        }
        entries = names.compactMap { url -> FileEntry? in
            let values = try? url.resourceValues(
                forKeys: [.isDirectoryKey, .fileSizeKey, .contentModificationDateKey])
            let isDir = values?.isDirectory ?? false
            let children = isDir
                ? (try? fm.contentsOfDirectory(atPath: url.path))?
                    .filter { !$0.hasPrefix(".") }.count ?? 0
                : 0
            return FileEntry(
                name: url.lastPathComponent,
                rel: relPath.isEmpty ? url.lastPathComponent : "\(relPath)/\(url.lastPathComponent)",
                isDir: isDir,
                size: Int64(values?.fileSize ?? 0),
                modified: values?.contentModificationDate,
                children: children
            )
        }
        .sorted {
            if $0.isDir != $1.isDir { return $0.isDir }
            return $0.name.lowercased() < $1.name.lowercased()
        }
    }

    private func show(_ entry: FileEntry) {
        let url = controller.dataDirectory.appendingPathComponent(entry.rel)
        // Same cap as the other GUI: a note is kilobytes, and a file this size
        // is a PDF the preview only needs to describe.
        let cap = 2 * 1024 * 1024
        guard let data = try? Data(contentsOf: url, options: .mappedIfSafe) else {
            problem = "The file could not be read."
            return
        }
        let slice = data.prefix(cap)
        if let text = String(data: slice, encoding: .utf8) {
            let markdown = ["md", "markdown"].contains(url.pathExtension.lowercased())
            preview = Preview(rel: entry.rel, size: entry.size, text: text,
                              markdown: markdown, truncated: data.count > cap)
        } else {
            preview = Preview(rel: entry.rel, size: entry.size, text: nil,
                              markdown: false, truncated: false)
        }
    }
}

struct FileEntry: Identifiable {
    var id: String { rel }
    let name: String
    let rel: String
    let isDir: Bool
    let size: Int64
    let modified: Date?
    let children: Int

    var detail: String {
        if isDir { return "\(children) item\(children == 1 ? "" : "s")" }
        var parts = [Self.human(size)]
        if let modified {
            // A file saved seconds ago formats as "in 0 seconds". The relative
            // formatter rounds toward the future for near-zero intervals.
            parts.append(Date().timeIntervalSince(modified) < 90
                ? "just now"
                : modified.formatted(.relative(presentation: .numeric)))
        }
        return parts.joined(separator: " · ")
    }

    static func human(_ bytes: Int64) -> String {
        let mb = Double(bytes) / 1_048_576
        if mb >= 1 { return String(format: "%.1f MB", mb) }
        if bytes >= 1024 { return "\(bytes / 1024) KB" }
        return "\(bytes) B"
    }
}

struct Preview {
    let rel: String
    let size: Int64
    /// nil means binary.
    let text: String?
    let markdown: Bool
    let truncated: Bool
}

struct PreviewView: View {
    let preview: Preview

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 10) {
                    Text(FileEntry.human(preview.size))
                    if preview.truncated { Text("showing the first 2 MB") }
                    Spacer()
                }
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)

                if let text = preview.text {
                    if preview.markdown {
                        // Whole-document markdown, paragraph by paragraph.
                        // AttributedString(markdown:) alone joins everything
                        // into one line; per-block parsing keeps structure.
                        MarkdownText(source: text)
                    } else {
                        Text(text)
                            .font(.system(size: 12, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                } else {
                    Text("This isn't a text file. It's \(FileEntry.human(preview.size)). It syncs like everything else. Open it from the folder.")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// Markdown rendering that keeps block structure.
///
/// `AttributedString(markdown:)` handles inline styling well but treats the
/// whole input as one paragraph. Splitting into blocks first, headings,
/// fences, lists, quotes, and styling each keeps a document readable without
/// taking a rendering dependency.
private struct MarkdownText: View {
    let source: String

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                render(block)
            }
        }
        .textSelection(.enabled)
    }

    private enum Block {
        case heading(Int, String)
        case code(String)
        case quote(String)
        case list([Item], ordered: Bool)
        case table(header: [String], rows: [[String]])
        case rule
        case para(String)
    }

    /// A list item, and whether it is a checklist entry.
    struct Item {
        let text: String
        /// nil for an ordinary bullet; true/false for a ticked or unticked box.
        let checked: Bool?
    }

    /// A pipe table row. The separator row beneath the header (|---|:--:|) is
    /// what proves a table rather than a line that happens to contain pipes.
    private static func isRow(_ t: String) -> Bool {
        let s = t.trimmingCharacters(in: .whitespaces)
        return s.hasPrefix("|") && s.hasSuffix("|") && s.count > 1
    }

    private static func isSeparator(_ t: String) -> Bool {
        guard isRow(t) else { return false }
        return cells(t).allSatisfy {
            $0.range(of: #"^:?-{2,}:?$"#, options: .regularExpression) != nil
        }
    }

    private static func cells(_ t: String) -> [String] {
        var s = t.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("|") { s.removeFirst() }
        if s.hasSuffix("|") { s.removeLast() }
        return s.components(separatedBy: "|").map { $0.trimmingCharacters(in: .whitespaces) }
    }

    private var blocks: [Block] {
        var out: [Block] = []
        var code: [String]? = nil
        var list: [Item] = []
        var ordered = false
        var para: [String] = []

        func flushPara() {
            if !para.isEmpty { out.append(.para(para.joined(separator: " "))); para = [] }
        }
        func flushList() {
            if !list.isEmpty { out.append(.list(list, ordered: ordered)); list = [] }
        }

        let allLines = source.components(separatedBy: "\n")
        var index = 0
        while index < allLines.count {
            let line = allLines[index]
            defer { index += 1 }
            if line.hasPrefix("```") {
                flushPara(); flushList()
                if let body = code { out.append(.code(body.joined(separator: "\n"))); code = nil }
                else { code = [] }
                continue
            }
            if code != nil { code?.append(line); continue }

            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty { flushPara(); flushList(); continue }

            if let match = trimmed.range(of: #"^#{1,6} "#, options: .regularExpression) {
                flushPara(); flushList()
                let level = trimmed.prefix(while: { $0 == "#" }).count
                out.append(.heading(level, String(trimmed[match.upperBound...])))
                continue
            }
            if trimmed.range(of: #"^(-{3,}|\*{3,})$"#, options: .regularExpression) != nil {
                flushPara(); flushList(); out.append(.rule); continue
            }
            // Tables, before the list rules: without this an 80-line reference
            // document renders mostly as literal pipes.
            if Self.isRow(line), index + 1 < allLines.count, Self.isSeparator(allLines[index + 1]) {
                flushPara(); flushList()
                let header = Self.cells(line)
                var rows: [[String]] = []
                var scan = index + 2
                while scan < allLines.count, Self.isRow(allLines[scan]) {
                    rows.append(Self.cells(allLines[scan]))
                    scan += 1
                }
                out.append(.table(header: header, rows: rows))
                index = scan - 1
                continue
            }
            if let match = trimmed.range(of: #"^[-*] "#, options: .regularExpression) {
                flushPara()
                if !list.isEmpty && ordered { flushList() }
                ordered = false
                list.append(Self.item(String(trimmed[match.upperBound...])))
                continue
            }
            if let match = trimmed.range(of: #"^\d+[.)] "#, options: .regularExpression) {
                flushPara()
                if !list.isEmpty && !ordered { flushList() }
                ordered = true
                list.append(Self.item(String(trimmed[match.upperBound...])))
                continue
            }
            if trimmed.hasPrefix(">") {
                flushPara(); flushList()
                out.append(.quote(String(trimmed.dropFirst()).trimmingCharacters(in: .whitespaces)))
                continue
            }
            para.append(trimmed)
        }
        flushPara(); flushList()
        if let body = code { out.append(.code(body.joined(separator: "\n"))) }
        return out
    }

    /// `- [ ]` and `- [x]`: a checklist is a list whose whole point is the
    /// boxes, and literal brackets lose it.
    private static func item(_ text: String) -> Item {
        guard let r = text.range(of: #"^\[[ xX]\] "#, options: .regularExpression) else {
            return Item(text: text, checked: nil)
        }
        let mark = text[text.index(text.startIndex, offsetBy: 1)]
        return Item(text: String(text[r.upperBound...]), checked: mark != " ")
    }

    @ViewBuilder private func render(_ block: Block) -> some View {
        switch block {
        case .heading(let level, let text):
            inline(text)
                .font(.system(size: level == 1 ? 19 : level == 2 ? 16 : 14, weight: .semibold))
                .padding(.top, 5)
        case .code(let body):
            Text(body)
                .font(.system(size: 11.5, design: .monospaced))
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 6).fill(.quaternary.opacity(0.4)))
        case .quote(let text):
            HStack(spacing: 8) {
                RoundedRectangle(cornerRadius: 1).fill(.quaternary).frame(width: 3)
                inline(text).font(.system(size: 12.5)).foregroundStyle(.secondary)
            }
        case .list(let items, let ordered):
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                    HStack(alignment: .top, spacing: 6) {
                        if let checked = item.checked {
                            Image(systemName: checked ? "checkmark.square.fill" : "square")
                                .font(.system(size: 12))
                                .foregroundStyle(checked ? Color.accentColor : .secondary)
                        } else {
                            Text(ordered ? "\(index + 1)." : "•")
                                .font(.system(size: 12.5))
                                .foregroundStyle(.secondary)
                        }
                        inline(item.text)
                            .font(.system(size: 12.5))
                            .foregroundStyle(item.checked == true ? .secondary : .primary)
                            .strikethrough(item.checked == true)
                    }
                }
            }
        case .table(let header, let rows):
            TableBlock(header: header, rows: rows)
        case .rule:
            Divider()
        case .para(let text):
            inline(text).font(.system(size: 12.5))
        }
    }

    private func inline(_ text: String) -> Text {
        if let attributed = try? AttributedString(
            markdown: text,
            options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        ) {
            return Text(attributed)
        }
        return Text(text)
    }
}

/// A markdown table.
///
/// Rows are HStacks in a VStack rather than a SwiftUI `Grid`. Grid could not be
/// trusted to size a row to its tallest cell here: inside a horizontally
/// scrolling ScrollView the proposed width is unbounded, so a cell that wrapped
/// to three lines still reported a one-line row, and its text spilled over the
/// header above and the row below with the last line clipped.
///
/// A fixed column width makes wrapping deterministic, and each cell then
/// stretches to the row's height so its border and background cover the whole
/// cell instead of stopping where its own text happens to end.
private struct TableBlock: View {
    let header: [String]
    let rows: [[String]]

    private var columns: Int { max(header.count, rows.map(\.count).max() ?? 0) }

    /// Wide enough for a sentence, narrow enough that three columns still fit
    /// a default window before the horizontal scroll is needed.
    private let columnWidth: CGFloat = 190

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            VStack(spacing: 0) {
                line(header, head: true)
                ForEach(Array(rows.enumerated()), id: \.offset) { _, cells in
                    line(cells, head: false)
                }
            }
            .overlay(RoundedRectangle(cornerRadius: 5).stroke(.quaternary, lineWidth: 1))
            .padding(.vertical, 3)
        }
    }

    private func line(_ cells: [String], head: Bool) -> some View {
        HStack(alignment: .top, spacing: 0) {
            ForEach(0..<columns, id: \.self) { index in
                cell(cells.indices.contains(index) ? cells[index] : "", head: head)
            }
        }
        // The row is exactly as tall as its tallest cell, and no taller.
        .fixedSize(horizontal: false, vertical: true)
    }

    private func cell(_ text: String, head: Bool) -> some View {
        // Inline styling still applies inside a cell: **bold** carries a lot of
        // meaning in a comparison table, which is most of what tables are for.
        let styled = (try? AttributedString(
            markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        )) ?? AttributedString(text)

        return Text(styled)
            .font(.system(size: 11.5, weight: head ? .semibold : .regular))
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .frame(width: columnWidth, alignment: .topLeading)
            // Stretch to the row, so the border and shading describe the cell
            // rather than the text that happens to be in it.
            .frame(maxHeight: .infinity, alignment: .topLeading)
            .background(head ? AnyShapeStyle(.quaternary.opacity(0.5)) : AnyShapeStyle(.clear))
            .border(.quaternary, width: 0.5)
    }
}


// MARK: - A note over time
//
// History, undelete and conflicts are one idea: versions of a note. They are
// sheets rather than windows because they are about the thing in front of you,
// where settings is about the app and gets a window of its own.

/// Sheets take an Identifiable, and a bare path is not one.
private struct HistoryTarget: Identifiable {
    let rel: String
    var id: String { rel }
}

private struct HistorySheet: View {
    let rel: String
    @EnvironmentObject private var controller: JotbayController
    @Environment(\.dismiss) private var dismiss

    @State private var versions: [Version] = []
    @State private var loading = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SheetHeader(title: "History", subtitle: rel) { dismiss() }
            Divider()

            if loading {
                EmptyPane(symbol: "clock", title: "Reading history", detail: "")
            } else if versions.isEmpty {
                EmptyPane(symbol: "clock", title: "No history yet",
                          detail: "This note has not been synced, so there is nothing to go back to.")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(versions.enumerated()), id: \.element.id) { index, v in
                            HStack(spacing: 10) {
                                VStack(alignment: .leading, spacing: 2) {
                                    HStack(spacing: 6) {
                                        Text(v.at, format: .dateTime.month().day().hour().minute())
                                            .font(.system(size: 12))
                                        if index == 0 {
                                            Text("current")
                                                .font(.system(size: 9, weight: .semibold))
                                                .foregroundStyle(.secondary)
                                        }
                                        if v.deleted {
                                            Text("deleted")
                                                .font(.system(size: 9, weight: .semibold))
                                                .foregroundStyle(.orange)
                                        }
                                    }
                                    Text([v.machine, v.short].compactMap { $0 }.joined(separator: " · "))
                                        .font(.system(size: 10))
                                        .foregroundStyle(.secondary)
                                }
                                Spacer()
                                // Not offered for the version already on disk:
                                // restoring it would be a no-op that still
                                // looks like it did something.
                                if index > 0 {
                                    Button("Restore") {
                                        controller.restore(rel, version: v.sha)
                                        dismiss()
                                    }
                                    .font(.system(size: 11))
                                }
                            }
                            .padding(.horizontal, 20)
                            .padding(.vertical, 9)
                            Divider().padding(.leading, 20)
                        }
                    }
                }
                Divider()
                Text("Restoring writes the old text back as an ordinary change. Nothing is rewritten, and the newer version stays in the history.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 10)
            }
        }
        .frame(width: 460, height: 460)
        .task {
            versions = await controller.history(of: rel)
            loading = false
        }
    }
}

private struct DeletedSheet: View {
    @EnvironmentObject private var controller: JotbayController
    @Environment(\.dismiss) private var dismiss

    @State private var gone: [DeletedNote] = []
    @State private var loading = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SheetHeader(title: "Deleted notes", subtitle: "Still in the history, and recoverable") {
                dismiss()
            }
            Divider()

            if loading {
                EmptyPane(symbol: "trash", title: "Looking", detail: "")
            } else if gone.isEmpty {
                EmptyPane(symbol: "trash", title: "Nothing has been deleted",
                          detail: "Notes removed on any machine would show up here.")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(gone) { d in
                            HStack(spacing: 10) {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(d.path).font(.system(size: 12))
                                    Text([d.machine, "removed"].compactMap { $0 }.joined(separator: " · "))
                                        .font(.system(size: 10))
                                        .foregroundStyle(.secondary)
                                }
                                Spacer()
                                Text(d.at, format: .dateTime.month().day())
                                    .font(.system(size: 10))
                                    .foregroundStyle(.tertiary)
                                Button("Restore") {
                                    controller.restore(d.path, version: nil)
                                    dismiss()
                                }
                                .font(.system(size: 11))
                            }
                            .padding(.horizontal, 20)
                            .padding(.vertical, 9)
                            Divider().padding(.leading, 20)
                        }
                    }
                }
            }
        }
        .frame(width: 480, height: 420)
        .task {
            gone = await controller.deletedNotes()
            loading = false
        }
    }
}

private struct NewNoteSheet: View {
    let done: () -> Void
    @EnvironmentObject private var controller: JotbayController
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SheetHeader(title: "New note", subtitle: nil) { dismiss() }
            Divider()
            VStack(alignment: .leading, spacing: 10) {
                TextField("Name", text: $name)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit(create)
                Text("Created in your notes folder, and opened in your editor. Without an extension it becomes a .md file.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                HStack {
                    Spacer()
                    Button("Cancel") { dismiss() }
                    Button("Create", action: create)
                        .keyboardShortcut(.defaultAction)
                        .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
            .padding(20)
        }
        .frame(width: 420)
    }

    private func create() {
        let wanted = name.trimmingCharacters(in: .whitespaces)
        guard !wanted.isEmpty else { return }
        controller.createNote(wanted) { ok in
            if ok {
                // Straight into the editor: creating a note and then having to
                // find it is most of the friction this is meant to remove.
                let named = wanted.contains(".") ? wanted : wanted + ".md"
                controller.openInEditor(named)
                done()
                dismiss()
            }
        }
    }
}

private struct SheetHeader: View {
    let title: String
    let subtitle: String?
    let close: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 13, weight: .semibold))
                if let subtitle {
                    Text(subtitle)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
            }
            Spacer()
            Button("Done", action: close).font(.system(size: 11))
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }
}
