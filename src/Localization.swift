import Foundation

enum UILang: String, CaseIterable {
    case ru, en
    var title: String { self == .ru ? "Рус" : "Eng" }
}

/// Язык интерфейса. Держим глобально, чтобы `tr()` можно было звать откуда угодно,
/// в том числе из фоновых потоков, без обращения к модели.
var currentUILang: UILang = .ru

/// Пара строк: русская и английская.
func tr(_ ru: String, _ en: String) -> String {
    currentUILang == .ru ? ru : en
}
