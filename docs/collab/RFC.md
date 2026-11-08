# RFC: совместное редактирование PDF в PdfCraft

- Статус: Implemented, draft PR
- Автор: `pdfcraft/envoy` (по ADR-0001…0010 и `ARCHITECTURE.md`)
- ADR: `.craft/bus/decisions/0001-op-unit.md` … `0010-undo-offline.md`
- Upstream issue/PR: ветка `collab/impl`; PR открывается отдельно
- Реализация: `crates/collab-{proto,server,pdf,hub,client,client-pdf,session,ws}`,
  `apps/pdfcraft-collab-server`, UI за фичей `collab`
- Основание: `TASK.md`, `INTEGRATION.md`, `docs/collab/study.md`, `docs/collab/options.md`

## 1. Контекст

В PdfCraft нет совместной работы: один пользователь, один файл. Документ — граф
COS-объектов с copy-on-write оверлеем (`crates/cos/src/document.rs:36-80`);
единственная точка правки — `Session::apply(DocId, Edit)` (`crates/engine/src/session/editing.rs:22`).
`Edit` — внутренний enum на 106 вариантов, не serde, адресует страницы
индексами, аннотации — `(page, index)`; публичный API не отдаёт `ObjRef`.
Undo — глобальный снапшот всего документа (`crates/engine/src/history.rs:8,106`).
После каждой правки движок пишет incremental-ревизию и создаёт новый
`RenderPool` (`editing.rs:332,392`); на 100 000 страниц реинспекция ≈5 с
(`docs/gaps.md:315-317`).

Нужны два класса действий (TASK.md):

1. **merge** — комментарии/аннотации, ответы, статусы, значения полей формы;
2. **structural** — rotate/delete/insert/move/duplicate/replace страниц,
   закладки, структура полей.

## 2. Цели и не-цели

Цели:

- 2–5 участников на один PDF; комментарии и аннотации в реальном времени;
- структурные операции сериализованы сервером и не теряют страницы;
- роли `owner/editor/commenter/viewer`, проверка на сервере;
- per-user undo, reconnect, commit в кабинет;
- одиночный режим не ломается; фича за флагом.

Не-цели v1:

- зашифрованные и подписанные PDF (сессия запрещена);
- юридически значимая ЭП, OCR-очередь, мобильный touch;
- офлайн-редактирование structural-операций;
- CRDT/OT.

## 3. Модель данных для синхронизации

### 3.1 Единица операции (ADR-0001)

Wire-формат — не `Edit`, а нормализованная intent-операция:

```text
Op {
  op_id: string,          // UUID клиента, дедуп на сервере
  doc: DocKey,            // file id кабинета + версия файла
  base_version: u64,      // версия, на которой клиент строил операцию
  epoch: u64,             // PDF: меняется structural-операцией
  kind: command | primitive | structural,
  class: merge | structural,   // назначает адаптер, не клиент
  target: Target,
  payload: Value,         // схема операции; blob — по sha256
  preview: Option<string> // id локального оптимистичного результата
}
```

- `command` — нормализованная команда движка (без `Selection`);
- `primitive` — узкая операция (например, правка текста аннотации);
- `structural` — операция над страницами/деревом.

Реализованные действия v1: `annotation.add` (только sticky note), `annotation.set_contents`,
`annotation.reply`, `annotation.delete`, `field.set_value`, `page.rotate`, `page.move`,
`page.delete`. Остальные правки в общей сессии отклоняются как `unsupported`, а не
применяются только локально.

### 3.2 Идентификаторы (ADR-0002)

| Сущность | Адрес на время сессии |
|---|---|
| страница | `page` + `epoch`; `epoch++` на каждую structural |
| аннотация | `/NM` (UUID, префикс `clientId`) + `page` + `epoch`; сервер отвергает дубли |
| ответ в thread | `/IRT` + `/NM` корня; сервер хранит карту |
| поле формы | полное имя `parent.child` |
| вложение | ключ name-tree |
| закладка | путь + `epoch` |
| слой | `ObjRef` OCG `(num, generation)` |

`ObjRef` страницы/аннотации — внутренний идентификатор сервера; на провод он не
попадает. Узкий API движка (`page_ref`, `annotation_ref`) реализован
(`de98b6e3`), но wire-формат от него не зависит.

`/NM` назначает клиент: `annotation.add`/`annotation.reply` несут поле `nm`,
сервер проверяет уникальность и сохраняет его (`Edit::AddAnnotationNamed`,
`Edit::ReplyToAnnotationNamed`). Если поле не передано, имя генерирует движок и
возвращает его в `applied.op.payload.nm`; клиент обязан применять именно его.

### 3.3 Документ на сервере

Авторитетная копия = `pdfcraft_engine::Session` с одним документом + журнал
операций + снапшоты (ADR-0005). Журнал хранит `opId`, `version`, `epoch`,
`actor`, `class`, адрес и payload; результат применения — не хранится, его
можно пересчитать.

## 4. Протокол

### 4.1 Транспорт

WebSocket/WSS, один набор сообщений для всех пяти приложений. JSON-фрейм
(текст), лимит одного сообщения — 256 KiB; blob-поля передаются отдельным
бинарным каналом или ссылкой `sha256`. Native-клиент: `tungstenite`;
web-клиент: `web-sys` WebSocket **или** host-мост `postMessage`
(`apps/pdfcraft-web/src/main.rs:120-160`) — выбирается в реализации, конверт
один.

### 4.2 Сообщения

| Тип | Направление | Поля | Семантика |
|---|---|---|---|
| `hello` | C→S | `v`, `app`, `doc`, `jwt`, `client` | JWT только здесь, не в URL |
| `welcome` | S→C | `sessionId`, `actorId`, `role`, `lease`, `version`, `epoch`, `limits` | сессия открыта |
| `op` | C→S | см. §3.1 | намерение |
| `applied` | S→C | `opId`, `version`, `epoch`, `actorId`, `op`, `result` | авторитетный результат; `op` — принятое намерение с нормализованным автором и `/NM`, по нему реплики применяют правку |
| `rejected` | S→C | `opId`, `code`, `retryable`, `hint{version,epoch}` | отказ |
| `presence` | C↔S | `page`, `viewport`, `cursor`, `selection`, `role`, `color` | эфемерно, ≤2 KiB, ≤10 Гц |
| `resync` | C→S | `lastVersion` | догон после разрыва |
| `snapshot` | S→C | `mode`, `cursor`, `bytes`/`ops` | база + хвост |
| `commit` / `commit-result` | C→S / S→C | см. §8 | сборка файла |
| `ping`/`pong` | C↔S | `ts` | heartbeat, продление lease |

Ошибки: `auth`, `lease`, `stale`, `conflict`, `forbidden`, `invalid`, `limit`,
`unsupported`.

### 4.3 Порядок и версии (ADR-0003)

- Один серверный монотонный `version` на документ; единственный порядок.
- `opId` дедуплицируется; окно последних операций персистентно (переживает
  рестарт) — повтор возвращает сохранённый `applied`, операция не применяется
  второй раз.
- `structural` требует `baseVersion == head`; иначе `rejected{code:"stale",
  retryable:true, hint:{version}}`.
- `merge` принимается на любой базе; при `epoch != head.epoch` —
  `rejected{code:"stale", retryable:true}`.
- Presence не увеличивает `version` и не журналируется.

### 4.4 Конфликты (ADR-0004)

| Ситуация | Политика | Что видит пользователь |
|---|---|---|
| два ответа в один thread | оба применяются, порядок серверный | оба ответа |
| правка текста/rect/цвета одной аннотации | LWW по свойству, побеждает серверный порядок | последнее значение + «изменено другим» |
| удаление аннотации с ответами | удаляются ответы и pop-up | thread исчез |
| ответ на удалённую аннотацию | `rejected{conflict}` | «комментарий удалён» |
| structural при активных участниках | эксклюзивно, `baseVersion == head` | «страницы изменены, обновите» |
| два structural одновременно | один применяется, второй stale | повтор после resync |
| значения полей | LWW по полному имени | последнее значение |
| структурная правка полей | только structural | «форма изменена» |

OT/CRDT не используются. LWW запрещён для содержимого (формулы, геометрия
страниц, текст документа) — только для свойств аннотаций и значений полей.

### 4.5 Reconnect и догон

Клиент присылает `lastVersion`. Если хвост журнала начиная с `lastVersion+1`
в окне — сервер отдаёт `applied`-последовательность; иначе `snapshot`
(база + хвост) и клиент сбрасывает непринятую очередь. Клиентские ops с
`opId`, уже применёнными, не повторяются (дедуп).

### 4.6 Снапшоты и compaction (ADR-0005)

- База — рабочие байты PDF (`Session::save_bytes`), хвост — журнал операций.
- Периодический snapshot: `write_incremental` от текущей базы; compaction по
  N операций / M байт / T минут сводит журнал в новую базу (допустим full
  rewrite только на компакции).
- Хранится 1–2 базы + хвост; TTL сессии — по правилу кабинета.
- Per-user undo-журнал хранится отдельно и в снапшот не входит.

## 5. Presence (ADR-0006)

Отдельный тип в том же WS; не журналируется. Полезная нагрузка: `page`
(индекс + epoch), `viewport {x,y,zoom}`, `cursor` (user space), `selection`
(`/NM` или rect), `role`, `color`. Частота ≤10 Гц, размер ≤2 KiB, при
backpressure — drop-stale, TTL 30 с. На wasm рендер идёт в UI-потоке
(`crates/render/src/raster/pool.rs:261-283`), поэтому presence не должен
вызывать перерисовку страницы.

## 6. Undo/redo на пользователя (ADR-0010)

- Undo — обычная операция с `baseVersion = head`, но с `inverse`-payload.
- Для аннотаций inverse: удалить созданное, восстановить прежние
  `Contents/Rect/State/Color` из серверного снимка свойств.
- Для structural inverse: обратный move/rotate, восстановление удалённых
  страниц из сохранённого объекта (сервер хранит копию до операции).
- Undo не откатывает чужие правки: он применяется поверх head и получает
  новую `version`; при конфликте — `rejected{conflict}`.
- Стек undo — на актора, на сессию; TTL истории — по TTL сессии.
- Offline: только `merge`-операции с `opId`; structural offline запрещены.

## 7. Роли и права (ADR-0008)

JWT в `hello`: `{sub, doc, role, sid, exp}`; `role ∈ owner|editor|commenter|viewer`.
Проверка на сервере для каждой операции:

- `viewer` — только presence/snapshot;
- `commenter` — merge-операции аннотаций/полей, без structural и без
  `Edit::Protect/ApplyRedactions/Sanitize`;
- `editor` — всё, кроме управления сессией;
- `owner` — всё + закрытие сессии.

Дополнительно проверяются PDF-permissions (`crates/engine/src/edit/run.rs:9-140`).
Токен не логируется; `Origin` проверяется для web; lease на документ
(один editor), продление по `ping`, отзыв — разрыв по `sid`.

## 8. Commit и версии (ADR-0009)

- Commit инициирует owner/editor или сервер по TTL/выходу последнего.
- Сервер применяет журнал к своей базе и пишет файл: `save_bytes`
  (incremental) по умолчанию; `save_full_bytes` только если операция требует
  полного rewrite (delete pages, redaction, PDF/A, смена шифрования).
  `/ModDate` инжектируется (`Session::with_clock`) — commit детерминирован.
- Кабинет получает `{fileId, bytes, sha256, version, expectedVersion}` и
  создаёт новую версию; исходная сохраняется.
- Внешнее изменение файла (etag/`file_watch`) → `409`, сессия не
  перезаписывает; кабинет предлагает «сохранить копию».
- После commit сессия жива, `dirty=false`, `version` не сбрасывается.

## 9. Сервер

- `pdfcraft-collab-server` — отдельный native-бинарь (не режим приложения).
- Зависимости: `pdfcraft-engine` (headless, без egui), `pdfcraft-collab-proto`,
  WebSocket-стек, JWT, storage. Лицензии — по allowlist upstream.
- Комната = `(fileId, app)`; воркер на комнату; падение комнаты не роняет
  чужие.
- Healthcheck, метрики (сессии, ops/s, размер журнала, lag), лимиты
  (участники, сообщение, ops/s, размер документа), TTL.
- Логи без содержимого PDF, комментариев и токенов.

## 10. Клиент

- Feature flag `collab` (default off), включается кабинетом через
  `pdfcraft:session` (host-мост) или query-параметр `collab=1`.
- Интеграция: `Session` остаётся источником истины для локального применения;
  collab-слой добавляет `apply_remote(op)` и отдельный путь применения чужих
  операций мимо `push_undo` (`editing.rs:188`).
- UI: список участников, курсоры/выделения, индикатор синхронизации; viewer и
  commenter не видят запрещённых команд (и сервер их отвергает).
- Одиночный режим: те же `save_bytes`, `render`, undo; collab не влияет.

## 11. Тесты

Headless, без GUI, 2–3 клиента:

1. unit: сериализация конверта, дедуп `opId`, `baseVersion`/`epoch`.
2. два клиента комментируют разные страницы — оба комментария видны.
3. два ответа в один thread — оба сохраняются, порядок стабилен.
4. правка/удаление одной аннотации — LWW и каскад ответов.
5. два structural одновременно — один stale, после resync применяется.
6. structural + merge на одной странице — merge отклоняется по epoch.
7. reconnect: `lastVersion` в окне и вне окна (snapshot).
8. роли: viewer/commenter не могут structural; сервер отвергает.
9. формы: значения полей LWW, структура — только structural.
10. undo: отмена своей правки не откатывает чужую.
11. commit: `.pdf` открывается в Chrome/Adobe; ссылки, закладки, формы,
    вложения целы; исходная версия сохранена.
12. большой PDF (500+ страниц) и 100k страниц — бюджет применения.

## 12. Безопасность

- JWT только в `hello`; не в URL, не в логах.
- `Origin` для web; WSS; размер сообщения и число ops/s ограничены.
- Сервер не логирует `Contents`, `T`, `payload` аннотаций, байты PDF.
- Server-side проверка роли и PDF-permissions; не полагаться на UI.
- Зашифрованные и подписанные PDF — сессия запрещена в v1.

## 13. Открытые вопросы

1. ~~Узкий API `engine` для `ObjRef`~~ — `page_ref`/`annotation_ref` есть;
   `epoch` оставлен как консервативная защита адресов страниц.
2. Headless-применение без пересборки view — **открыт**: `apply_remote`
   обновляет `RenderPool` на каждую правку; батч на окно commit не сделан.
3. ~~Транспорт web~~ — native: блокирующий WS; web: host-мост (не подключён).
4. ~~Судьба `/NM`~~ — клиент назначает, сервер проверяет и сохраняет; при
   отсутствии имени движок генерирует и возвращает его в `applied.op`.
5. ~~Snapshot/compaction и TTL~~ — commit пишет рабочую копию (атомарно), TTL
   сессии не реализован; журнал между рестартами **не сохраняется**.
6. ~~Лимиты~~ — `max_participants` и `max_ops_per_sec` проверяются комнатой;
   ограничение размера документа не задано.
7. **Не разделяются в v1**: произвольные аннотации (highlight/shape/ink/stamp/
   text box), вставка/дублирование/замена страниц, закладки, слои, вложения,
   изменение содержимого страницы, повтор/redo структурных правок. Клиент
   показывает «not shared yet» и не применяет их локально.

## 14. План поставки и состояние

1. **PR-1:** `collab-proto` — конверт, типы, валидация. Готово (`54bfd810`).
2. **PR-2:** узкий API движка для стабильных ID. Готово (`de98b6e3`).
3. **PR-3:** комната (`collab-server`), PDF-адаптер (`collab-pdf`), hub
   (`collab-hub`). Готово (`a3ebb809`, `2068b4b0`, `0c7ad5e3`,
   `63025edf`, `96a4916b`).
4. **PR-4:** клиент: `apply_remote` (`6ecd94f0`), машина состояний
   (`5a6dcb26`), проекция (`a5120280`), фоновый воркер (`02f45cdb`), UI за
   фичей `collab` (в этой ветке).
5. **PR-5:** транспорт WS+JWT (`4f7ea0c2`), серверный бинарь
   `apps/pdfcraft-collab-server`, commit в рабочую копию; e2e на 3 клиента
   (`5f2a8d7c`); большой PDF — TODO.
6. **PR-6:** документация и флаг по умолчанию off — этот раздел и `--features
   collab`.

Северная звезда prod: кабинет поднимает сервер, кладёт `fileId.pdf` в
`--documents`, выдаёт короткий JWT; клиент открывает iframe/нативное окно с
`PDFCRAFT_COLLAB_URL/TOKEN/DOC`.

## 15. Запуск сервера

```bash
cargo run -p pdfcraft-collab-daemon --release -- \
  --documents /srv/collab/documents --data /srv/collab/data \
  --jwt-key /srv/collab/cabinet-public.pem --jwt-audience lzmkm \
  --listen 127.0.0.1:8787 --health 127.0.0.1:8788
```

- одна комната на `<fileId>.pdf`; рабочая копия — `--data/<fileId>.pdf`;
- commit (кнопка, или последний участник ушёл при непустой сессии) пишет
  рабочую копию атомарно; исходный файл в `--documents` не меняется;
- `/health` → `{"ok":true,"rooms":N,"clients":M}`; `/metrics` — Prometheus-текст;
- логи не содержат токенов, текста комментариев и байтов PDF.

## 16. Известные ограничения (до prod)

1. Журнал операций не переживает рестарт: восстановление идёт с последней
   рабочей копии, правки после последнего commit теряются.
2. Web-клиент к серверу не подключён (нужен host-мост `pdfcraft:session`).
3. Presence: панель участников и страница/зум; курсоры и выделения не рисуются.
4. Undo покрывает обратимые операции (комментарии, текст, поворот, перенос,
   поля); удаление аннотации/страницы не отменяется.
5. Не разделяются произвольные аннотации и часть структурных операций (см. §13.7).
6. Комнаты заводятся каталогом файлов; control-plane кабинета (`POST /v1/sessions`)
   не реализован.
7. Нет WSS/TLS termination внутри сервера (ожидается reverse proxy).
