## Что нового в 2.1.0

**Разделение по голосам.** Включите «Разделять по голосам» под кнопками — приложение определит, кто что сказал, и подпишет реплики: «Спикер 1», «Спикер 2»… Нажмите на спикера над текстом и дайте ему имя — оно сразу заменится во всём тексте, в том числе в уже сохранённых .txt. Если знаете, сколько людей в записи, укажите — так точнее. Модели голосов (~34 МБ) скачиваются один раз при первом включении и работают без интернета.

**Обновления по воздуху.** При запуске приложение проверяет, не вышла ли новая версия, и предлагает «Скачать и установить» — на macOS, Windows и в AppImage для Linux. Обновления подписаны: чужой пакет приложение не поставит. Проверку можно выключить в «О программе».

**Старые процессоры.** На компьютерах с процессором без AVX2 (Core 2-го и 3-го поколения, Pentium, Celeron) Windows- и Linux-версии закрывались при начале расшифровки. Теперь в установщике есть совместимая копия движка, и приложение само переключается на неё.

---

## What's new in 2.1.0

**Speaker separation.** Turn on *Split by speaker* under the buttons and the app works out who said what, labelling lines “Speaker 1”, “Speaker 2”… Click a speaker above the text and give them a name — it replaces the label throughout the text, including .txt files already saved. If you know how many people speak, set it for better accuracy. The voice models (~34 MB) download once and work offline.

**Over-the-air updates.** At launch the app checks for a new version and offers *Download and install* on macOS, Windows and the Linux AppImage. Updates are signed, so a tampered package is refused. The check can be turned off in *About*.

**Older CPUs.** On processors without AVX2 (2nd/3rd-gen Core, Pentium, Celeron) the Windows and Linux builds closed as soon as transcription started. The installer now carries a compatible copy of the engine and the app switches to it automatically.
