import SwiftUI
import UniformTypeIdentifiers

struct ContentView: View {

    @ObservedObject private var model    = AppModel.shared
    @ObservedObject private var recorder = AppModel.shared.recorder
    @ObservedObject private var monitor  = AppModel.shared.monitor
    @State private var dropTargeted = false

    var body: some View {
        VStack(spacing: 14) {
            settingsBar
            dropZone
            if let error = model.errorMessage { errorBar(error) }
            if model.isBusy { progressBar }
            transcriptView
            bottomBar
            Divider()
            resourceBar
        }
        .padding(18)
        .frame(minWidth: 720, minHeight: 640)
        .onDrop(of: [UTType.fileURL], isTargeted: $dropTargeted, perform: handleDrop)
        .onAppear { monitor.start() }
        .onDisappear { monitor.stop() }
    }

    // MARK: - Настройки

    private var settingsBar: some View {
        HStack(spacing: 14) {
            Picker(tr("Язык", "Language"), selection: $model.language) {
                ForEach(model.languageOptions, id: \.code) { option in
                    Text(option.title).tag(option.code)
                }
            }
            .frame(width: 290)
            .disabled(model.isBusy)

            Spacer()

            Toggle(tr("Таймкоды", "Timestamps"), isOn: $model.timestamps)
                .help(tr("Добавлять [00:12 → 00:19] перед каждой фразой",
                         "Prefix every phrase with [00:12 → 00:19]"))
            Toggle(tr("Сохранять .txt рядом", "Save .txt alongside"), isOn: $model.autoSave)
                .help(tr("Класть готовый текст рядом с исходным файлом; диктовки — в Документы/Whisper",
                         "Put the text next to the source file; dictations go to Documents/Whisper"))

            Picker("", selection: $model.uiLang) {
                ForEach(UILang.allCases, id: \.self) { lang in
                    Text(lang.title).tag(lang)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 96)
            .help(tr("Язык интерфейса", "Interface language"))
        }
        .toggleStyle(.checkbox)
    }

    // MARK: - Зона перетаскивания и запись

    private var dropZone: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 12)
                .strokeBorder(style: StrokeStyle(lineWidth: dropTargeted ? 2.5 : 1.5, dash: [7, 5]))
                .foregroundStyle(dropTargeted ? Color.accentColor : Color.secondary.opacity(0.5))
                .background(
                    RoundedRectangle(cornerRadius: 12)
                        .fill(dropTargeted ? Color.accentColor.opacity(0.08) : Color.secondary.opacity(0.05))
                )
            if recorder.isRecording { recordingPanel } else { idlePanel }
        }
        .frame(height: 150)
    }

    private var idlePanel: some View {
        VStack(spacing: 10) {
            Image(systemName: "waveform")
                .font(.system(size: 30, weight: .light))
                .foregroundStyle(.secondary)
            Text(tr("Перетащите сюда аудио или видео", "Drop audio or video here"))
                .foregroundStyle(.secondary)
            HStack(spacing: 12) {
                Button {
                    model.openPanel()
                } label: {
                    Label(tr("Выбрать файл…", "Choose a file…"), systemImage: "folder")
                }
                .disabled(model.isBusy)

                Button {
                    model.toggleRecording()
                } label: {
                    Label(tr("Надиктовать", "Dictate"), systemImage: "mic.fill")
                }
                .disabled(model.isBusy)
            }
            .controlSize(.large)
        }
        .padding()
    }

    private var recordingPanel: some View {
        VStack(spacing: 12) {
            HStack(spacing: 10) {
                Circle().fill(.red).frame(width: 10, height: 10)
                Text(tr("Запись — ", "Recording — ") + Formatter2.timecode(recorder.duration))
                    .font(.system(.title3, design: .rounded).monospacedDigit())
            }
            levelMeter
            Button {
                model.toggleRecording()
            } label: {
                Label(tr("Остановить и расшифровать", "Stop and transcribe"), systemImage: "stop.fill")
            }
            .controlSize(.large)
            .keyboardShortcut(.return, modifiers: [])
        }
        .padding()
    }

    private var levelMeter: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Color.secondary.opacity(0.2))
                Capsule()
                    .fill(Color.accentColor)
                    .frame(width: max(4, geo.size.width * CGFloat(recorder.level)))
                    .animation(.linear(duration: 0.08), value: recorder.level)
            }
        }
        .frame(width: 260, height: 6)
    }

    // MARK: - Прогресс и ошибки

    private var progressBar: some View {
        HStack(spacing: 12) {
            ProgressView(value: model.progress).progressViewStyle(.linear)
            Text("\(Int(model.progress * 100))%")
                .font(.callout.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 44, alignment: .trailing)
            if !model.queueInfo.isEmpty {
                Text(model.queueInfo).font(.callout).foregroundStyle(.secondary)
            }
            Button(tr("Отмена", "Cancel")) { model.cancel() }
        }
    }

    private func errorBar(_ text: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            Text(text).font(.callout).textSelection(.enabled)
            Spacer()
            Button { model.errorMessage = nil } label: { Image(systemName: "xmark.circle.fill") }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 8).fill(Color.orange.opacity(0.12)))
    }

    // MARK: - Текст

    private var transcriptView: some View {
        ZStack(alignment: .topLeading) {
            TextEditor(text: $model.transcript)
                .font(.system(size: 13))
                .padding(6)
                .background(RoundedRectangle(cornerRadius: 8).fill(Color(nsColor: .textBackgroundColor)))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color.secondary.opacity(0.3)))

            if model.transcript.isEmpty {
                Text(tr("Здесь появится расшифровка. Текст можно править прямо тут.",
                        "The transcript will appear here. You can edit it right in place."))
                    .foregroundStyle(.tertiary)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 14)
                    .allowsHitTesting(false)
            }
        }
        .frame(minHeight: 230)
    }

    private var bottomBar: some View {
        HStack(spacing: 10) {
            Text(model.status)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .textSelection(.enabled)

            Spacer()

            if model.lastSavedURL != nil {
                Button { model.revealLastSaved() } label: {
                    Label(tr("Показать в Finder", "Show in Finder"), systemImage: "arrow.right.circle")
                }
            }
            Button(tr("Очистить", "Clear")) { model.clear() }
                .disabled(model.transcript.isEmpty)
            Button { model.copyToClipboard() } label: {
                Label(tr("Копировать", "Copy"), systemImage: "doc.on.doc")
            }
            .disabled(model.transcript.isEmpty)
            Button { model.saveAs() } label: {
                Label(tr("Сохранить .txt…", "Save .txt…"), systemImage: "square.and.arrow.down")
            }
            .disabled(model.transcript.isEmpty)
        }
    }

    // MARK: - Нагрузка

    private var resourceBar: some View {
        let s = monitor.stats
        let cores = Double(monitor.coreCount)
        return HStack(spacing: 18) {
            gauge(title: tr("ЦП приложения", "App CPU"),
                  value: s.appCPU / (cores * 100),
                  text: "\(Int(s.appCPU))%",
                  hint: tr("100 % = одно ядро целиком, всего ядер: \(monitor.coreCount)",
                           "100% = one full core, \(monitor.coreCount) cores total"))

            gauge(title: tr("ЦП системы", "System CPU"),
                  value: s.systemCPU / 100,
                  text: "\(Int(s.systemCPU))%",
                  hint: tr("Загрузка всех ядер компьютера", "Load across all cores"))

            if s.hasGPU {
                gauge(title: "GPU",
                      value: s.gpu / 100,
                      text: "\(Int(s.gpu))%",
                      hint: tr("Видеоядро — на нём и считает модель", "The GPU — where the model actually runs"))
            }

            HStack(spacing: 6) {
                Text(tr("Память", "Memory")).foregroundStyle(.secondary).fixedSize()
                Text(memoryText(s.memoryMB)).monospacedDigit().fixedSize()
            }

            Spacer()

            HStack(spacing: 6) {
                Image(systemName: "cpu").foregroundStyle(.secondary)
                Text(WhisperEngine.modelDisplayName).foregroundStyle(.secondary).fixedSize()
            }
            .help(tr("Модель распознавания, вшитая в приложение",
                     "The recognition model built into the app"))
        }
        .font(.caption)
    }

    private func gauge(title: String, value: Double, text: String, hint: String) -> some View {
        HStack(spacing: 6) {
            Text(title).foregroundStyle(.secondary).fixedSize()
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color.secondary.opacity(0.18))
                    Capsule()
                        .fill(barColor(value))
                        .frame(width: max(2, geo.size.width * min(1, max(0, value))))
                        .animation(.linear(duration: 0.9), value: value)
                }
            }
            .frame(width: 64, height: 5)
            Text(text).monospacedDigit().frame(width: 38, alignment: .leading)
        }
        .help(hint)
    }

    private func barColor(_ value: Double) -> Color {
        switch value {
        case ..<0.6:  return .green
        case ..<0.85: return .yellow
        default:      return .orange
        }
    }

    private func memoryText(_ mb: Double) -> String {
        mb >= 1024 ? String(format: "%.2f ", mb / 1024) + tr("ГБ", "GB")
                   : String(format: "%.0f ", mb) + tr("МБ", "MB")
    }

    // MARK: - Drag & drop

    private func handleDrop(providers: [NSItemProvider]) -> Bool {
        let lock = NSLock()
        var urls: [URL] = []
        let group = DispatchGroup()

        for provider in providers {
            group.enter()
            provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier, options: nil) { item, _ in
                var found: URL?
                if let data = item as? Data { found = URL(dataRepresentation: data, relativeTo: nil) }
                else if let url = item as? URL { found = url }
                if let found { lock.lock(); urls.append(found); lock.unlock() }
                group.leave()
            }
        }
        group.notify(queue: .main) {
            if !urls.isEmpty { model.process(urls: urls) }
        }
        return true
    }
}
