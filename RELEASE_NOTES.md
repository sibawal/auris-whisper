## Что нового в 2.2.0

**Модели для русской речи.** В окне моделей появился раздел «Только для русской речи»:

- **GigaAM v3** от Сбера (221 МБ) — самая точная для русского. Сама ставит знаки препинания и заглавные буквы, пишет числа цифрами.
- **T-One** от Т-Банка (138 МБ) — обучена на телефонных разговорах, хороша для звонков и шумных записей. Пишет без знаков препинания.

Обе работают на процессоре и в несколько раз быстрее Whisper: на Apple M4 GigaAM v3 расшифровывает 17 минут речи меньше чем за минуту. На компьютерах без видеокарты разница ещё заметнее. Разделение по голосам, таймкоды и диктовка работают с ними так же, как с Whisper.

Другие языки эти модели не понимают, поэтому, пока выбрана одна из них, язык распознавания зафиксирован на русском. Если выбрать русский язык при модели Whisper, приложение предложит попробовать GigaAM.

**Linux.** В версии 2.1.0 разделение по голосам на Linux закрывало приложение — исправлено.

---

## What's new in 2.2.0

**Models for Russian speech.** The model window has a new *Russian speech only* section:

- **GigaAM v3** by Sber (221 MB) — the most accurate for Russian. Adds punctuation and capitals, writes numbers as digits.
- **T-One** by T-Bank (138 MB) — trained on phone calls, good for calls and noisy recordings. Writes without punctuation.

Both run on the CPU and are several times faster than Whisper: on an Apple M4, GigaAM v3 transcribes 17 minutes of speech in under a minute. On computers without a GPU the gap is even larger. Speaker separation, timestamps and dictation work with them just as with Whisper.

They do not understand other languages, so while one of them is selected the recognition language is locked to Russian. Pick Russian with a Whisper model and the app suggests trying GigaAM.

**Linux.** In 2.1.0, speaker separation closed the app on Linux — fixed.
