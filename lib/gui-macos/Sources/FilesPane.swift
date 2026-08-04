import SwiftUI

/// Read-only browser for the notes folder: one directory at a time, breadcrumb
/// back, markdown rendered on selection.
///
/// Reads the filesystem directly rather than shelling out to the CLI — this is
/// the one surface where the CLI has nothing to add: there is no git state
/// involved, and a preview that spawns a process per click feels like one.
struct FilesPane: View {
    @EnvironmentObject private var controller: JotbayController

    @State private var relPath = ""
    @State private var entries: [FileEntry] = []
    @State private var preview: Preview?
    @State private var problem: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            crumbs
            Divider()
            if let preview {
                PreviewView(preview: preview)
            } else if let problem {
                EmptyPane(symbol: "exclamationmark.triangle", title: "Can't read that",
                          detail: problem)
            } else if entries.isEmpty {
                EmptyPane(symbol: "tray", title: "Nothing here yet",
                          detail: "Files you put in this folder sync everywhere.")
            } else {
                listing
            }
        }
        .onAppear { load() }
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
            }
        }
    }

    // MARK: - Data

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
            // A file saved seconds ago formats as "in 0 seconds" — the relative
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
                    Text("Not a text file — \(FileEntry.human(preview.size)). It syncs like everything else; open it from the folder.")
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
/// whole input as one paragraph. Splitting into blocks first — headings,
/// fences, lists, quotes — and styling each keeps a document readable without
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
/// A Grid rather than SwiftUI's `Table`, which is a list control built around
/// a typed row model — the wrong shape entirely for cells parsed out of text.
/// Scrolls horizontally on its own so a wide reference table never widens the
/// pane it sits in.
private struct TableBlock: View {
    let header: [String]
    let rows: [[String]]

    private var columns: Int { max(header.count, rows.map(\.count).max() ?? 0) }

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Grid(alignment: .topLeading, horizontalSpacing: 0, verticalSpacing: 0) {
                GridRow {
                    ForEach(0..<columns, id: \.self) { column in
                        cell(header.indices.contains(column) ? header[column] : "", head: true)
                    }
                }
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    GridRow {
                        ForEach(0..<columns, id: \.self) { column in
                            cell(row.indices.contains(column) ? row[column] : "", head: false)
                        }
                    }
                }
            }
            .overlay(RoundedRectangle(cornerRadius: 5).stroke(.quaternary, lineWidth: 1))
            .padding(.vertical, 3)
        }
    }

    private func cell(_ text: String, head: Bool) -> some View {
        // Inline styling still applies inside a cell: **bold** carries a lot of
        // meaning in a comparison table, which is most of what tables are for.
        let styled = (try? AttributedString(
            markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        )) ?? AttributedString(text)

        return Text(styled)
            .font(.system(size: 11.5, weight: head ? .semibold : .regular))
            .frame(maxWidth: 260, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(head ? AnyShapeStyle(.quaternary.opacity(0.5)) : AnyShapeStyle(.clear))
            .border(.quaternary, width: 0.5)
    }
}
