## Что нового в 2.2.1

**Разделение по голосам переделано.**

- Число спикеров определяется верно. Раньше на живом разговоре двух человек приложение могло найти 5–10 «спикеров», а если число задать вручную — отдать весь текст одному.
- В 5–10 раз быстрее: 3,5 минуты разговора делятся по голосам меньше чем за 3 секунды на Apple M4.
- Короткие реплики на стыке («да», «угу») точнее попадают к своему спикеру: граница ставится в паузу между репликами.
- Нужна одна модель (28 МБ) вместо двух. Если разделение по голосам уже включали — ничего докачивать не придётся.

**Нумерация спикеров.** Каждая новая расшифровка начинает с «Спикер 1». Раньше номера продолжались от предыдущего файла в окне (3, 4…), и казалось, что спикеров больше, чем на самом деле.

---

## What's new in 2.2.1

**Speaker separation rebuilt.**

- The number of speakers is now detected correctly. Before, a live two-person conversation could come out as 5–10 "speakers", and setting the number by hand could hand all the text to one person.
- 5–10× faster: 3.5 minutes of conversation are split in under 3 seconds on an Apple M4.
- Short replies at a turn change ("yes", "uh-huh") land with the right speaker: the boundary is placed in the pause between replies.
- One model (28 MB) instead of two. If you have used speaker separation before, nothing new is downloaded.

**Speaker numbering.** Every new transcription starts again from "Speaker 1". Before, numbers continued from the previous file in the window (3, 4…), which looked like extra speakers.
