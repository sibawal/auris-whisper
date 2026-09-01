import SwiftUI
import AppKit

@main
struct AurisWhisperApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @ObservedObject private var model = AppModel.shared

    var body: some Scene {
        WindowGroup("Auris Whisper") {
            ContentView()
        }
        .commands {
            CommandGroup(replacing: .appInfo) {
                Button(tr("О программе Auris Whisper", "About Auris Whisper")) { showAbout() }
            }
            CommandGroup(replacing: .newItem) {
                Button(tr("Открыть аудио…", "Open audio…")) { AppModel.shared.openPanel() }
                    .keyboardShortcut("o", modifiers: [.command])
                Button(tr("Надиктовать", "Dictate")) { AppModel.shared.toggleRecording() }
                    .keyboardShortcut("r", modifiers: [.command])
            }
            CommandGroup(replacing: .saveItem) {
                Button(tr("Сохранить .txt…", "Save .txt…")) { AppModel.shared.saveAs() }
                    .keyboardShortcut("s", modifiers: [.command])
            }
            CommandGroup(replacing: .help) {
                Button(tr("Лицензия", "License")) { openLicense() }
            }
        }
    }

    private func showAbout() {
        let credits = tr("""
        Офлайн-расшифровка речи. Ничего не уходит в сеть — всё считается на вашем Mac.

        Разработчик: boopi.ru
        Открытая лицензия MIT: можно свободно использовать, изменять и распространять.

        Модель: \(WhisperEngine.modelDisplayName)
        Внутри: whisper.cpp и ggml (MIT), модель Whisper от OpenAI (MIT),
        libopus, libvorbis, libogg (BSD).
        """, """
        Offline speech transcription. Nothing leaves your Mac — everything runs locally.

        Developer: boopi.ru
        Open MIT license: free to use, modify and redistribute.

        Model: \(WhisperEngine.modelDisplayName)
        Built on: whisper.cpp and ggml (MIT), the Whisper model by OpenAI (MIT),
        libopus, libvorbis, libogg (BSD).
        """)

        let attributed = NSAttributedString(
            string: credits,
            attributes: [.font: NSFont.systemFont(ofSize: 11),
                         .foregroundColor: NSColor.labelColor])

        NSApplication.shared.activate(ignoringOtherApps: true)
        NSApplication.shared.orderFrontStandardAboutPanel(options: [.credits: attributed])
    }

    private func openLicense() {
        guard let url = Bundle.main.url(forResource: "LICENSE", withExtension: "txt") else { return }
        NSWorkspace.shared.open(url)
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func application(_ application: NSApplication, open urls: [URL]) {
        AppModel.shared.process(urls: urls)
    }
}
