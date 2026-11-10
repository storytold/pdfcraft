# PdfCraft collaboration server — эксплуатация

Бинарь `pdfcraft-collab-server` (пакет `pdfcraft-collab-daemon`), протокол —
[`RFC.md`](RFC.md) §8–9, состояние и тестовая матрица — [`STATUS.md`](STATUS.md).
Документ описывает, что сервер принимает снаружи, что отдаёт и как выйти из
конфликта публикации.

## 1. Запуск

```bash
# прод: комнаты из control-plane кабинета
pdfcraft-collab-server \
  --data /srv/collab/data \
  --jwt-secret-file /srv/collab/cabinet-jwt-secret --jwt-audience lzmkm --app pdfcraft \
  --listen 127.0.0.1:8787 --health 127.0.0.1:8788 --control 127.0.0.1:8789 \
  --callback-url http://cabinet.internal/v1/collab/commit \
  [--max-source-bytes N] [--origin https://cabinet.example]

# dev: комнаты из каталога, без кабинета
pdfcraft-collab-server --documents /srv/collab/documents --data /srv/collab/data [...]
```

| Порт | Что | Авторизация |
|---|---|---|
| `--listen` | WebSocket-протокол (`hello` + session JWT) | JWT в первом сообщении |
| `--health` | `GET /health`, `GET /metrics` | нет (для probes) |
| `--control` | `POST /v1/sessions`, `POST /v1/sessions/{fileId}/resolve` | JWT в `Authorization: Bearer` |

TLS внутри нет: оба HTTP-порта и WebSocket ожидаются за reverse proxy
(`https://` в callback/URL источнике отклоняется явной ошибкой). Токены,
содержимое PDF и тексты комментариев не логируются.

JWT: **HS256** общим секретом кабинета (`--jwt-secret-file FILE` или
`PDFCRAFT_COLLAB_JWT_SECRET`, не короче 16 байт); **RS256** (`--jwt-key FILE` с
публичным ключом) остаётся опцией. Ровно один способ обязателен: ключ и секрет
одновременно (или два секрета) — ошибка старта, как и отсутствие обоих.
Секрет не логируется и не попадает в текст ошибки.

Claims: `aud` (обязателен, равен `--jwt-audience`), `sub` (непустой, актор),
`exp` (допуск 30 с), `role ∈ owner|editor|commenter|viewer`; имя файла — `doc`
или `fileId` (строка RFC или число кабинета), сессия — `sid` или `sessionId`;
`app`, если есть, равен `--app`. Имя файла обязано совпадать с `hello.doc` и с
файлом в control-plane. Подробнее — RFC §7.1.

## 2. Control plane

### 2.1. `POST /v1/sessions` — создать или возобновить комнату

Заголовок `Authorization: Bearer <session JWT>` (токен тот же, что уйдёт в
`hello`). Тело:

```json
{"fileId":"file-123","jwt":"<session JWT>","etag":"sha256:…",
 "bytes":"<base64 PDF>"}
```

вместо `bytes` допустим `"url": "http://…"`; `initialBytes` — синоним `bytes`.
Ответ `201` (создана) или `200` (возобновлена):
`{"fileId","version","epoch","etag","created"}`.

| Код | Когда |
|---|---|
| `400` | битое тело, не-PDF, `fileId` вне `[A-Za-z0-9._-]{1,128}`, etag ≠ хешу байтов |
| `401` | токен не проверяется |
| `403` | токен выписан на другой `fileId` |
| `404` | комнаты нет и источника нет («только возобновление») |
| `409` | у комнаты другой etag: файл изменился вне сессии |
| `413` | файл больше `--max-source-bytes` (256 МиБ) |
| `502` | источник по `url` не ответил |

### 2.2. `POST /v1/sessions/{fileId}/resolve` — снять `conflict`

Единственный способ вывести комнату из состояния `conflict` (см. §3).
Заголовок `Authorization: Bearer <service token>` — тот же session JWT кабинета,
что и у сессии; `fileId` токена обязан совпадать с путём, роль — `owner` или
`editor` (commenter/viewer файл не сохраняет, поэтому и конфликт не разрешает).

```json
{"expectedVersion": 7, "etag": "sha256:<64 hex>"}
```

`expectedVersion` — версия копии в кабинете, `etag` — её etag. Ответ `200`:

```json
{"fileId":"file-123","version":12,"epoch":1,
 "etag":"sha256:…","cabinetVersion":7,"conflict":false}
```

`version`/`epoch` — версия документа в комнате (resolve её не двигает),
`cabinetVersion` — принятая версия кабинета.

| Код | `code` | Когда |
|---|---|---|
| `400` | `invalid` | битое тело, etag не `sha256:<64 hex>`, `expectedVersion` не новее той, что комната уже знает |
| `401` | `auth` | нет или не проверяется токен |
| `403` | `forbidden` | токен на другой файл; роль без права записи |
| `404` | `missing` | комнаты для файла нет |
| `409` | `not_conflict` | комната не в конфликте (в т.ч. повторный resolve) |
| `409` | `busy` | идёт публикация этого документа; повторить |
| `503` | `unavailable` | недоступен hub |

Любой отказ состояние не меняет. Успешный resolve:

- снимает флаг `conflict` (`/health`, `/metrics`);
- записывает в маркер `CABINET`, что кабинет держит `expectedVersion`/`etag`;
- **сразу публикует** базу, которую кабинет до этого отклонил: комната не ждёт
  ни коммита, ни входа, ни ухода последнего участника. Уходит тот же payload
  (`version` комнаты, байты базы) и тот же ключ `fileId:version:sha256`, что и
  у отклонённой попытки, с `expectedVersion = expectedVersion` из запроса;
  `409` на неё снова помечает комнату `conflict` и останавливает очередь;
- оставляет журнальный хвост после этой базы `dirty`: его публикует следующий
  коммит (кнопка, уход последнего участника, серверная компакция);
- учитывается в `pdfcraft_collab_conflicts_resolved_total` и в
  `pdfcraft_collab_publications_after_resolve_total`.

## 3. Commit callback

`POST --callback-url`, токен — env `PDFCRAFT_COLLAB_CALLBACK_TOKEN`
(`Authorization: Bearer`, он же в `Idempotency-Key`):

```json
{"fileId":"file-123","bytes":"<base64 PDF>","sha256":"…",
 "version":7,"expectedVersion":6,
 "idempotencyKey":"file-123:7:…"}
```

- `idempotencyKey` = `fileId:version:sha256`; выводится, а не генерируется:
  повтор той же публикации (в т.ч. после рестарта демона) приходит с тем же
  ключом, и кабинет, принявший байты без ответа, узнаёт дубль.
- `2xx` — кабинет принял версию. Если тело ответа — `{"version","etag"}`, это
  и есть новая копия кабинета: её версия становится `expectedVersion`
  следующего коммита, etag — маркером. Пустое/нечитаемое тело — версия
  `expectedVersion + 1`.
- `409` — у кабинета другая версия: коммит отклонён, комната `conflict`,
  очередь публикаций останавливается (см. §4).
- сеть / `429` / `5xx` — комната остаётся `publishing`, фоновая очередь
  повторяет ту же публикацию (тот же `version`, `expectedVersion`, `sha256`,
  ключ) с удвоением паузы 1 с → 60 с; учитывается в
  `pdfcraft_collab_publication_retries_total`.
- прочие `4xx` — контракт неверен, повтор не запланирован; комната остаётся
  `dirty` до следующего коммита.

Публикация выполняется **вне** блокировки hub; второй коммит того же документа
во время публикации получает `busy` (`retryable`), поэтому две версии не
приходят в кабинет не по порядку.

## 3.1. Перезапуск демона

Очередь публикаций живёт в памяти, но её содержимое выводится из состояния на
диске: при старте демон обходит `--data`, и каждая комната, у которой база в
`CURRENT` новее базы в `CABINET` (коммит записал базу, а POST не дошёл или не
успел), открывается из своей базы и журнала и ставится в очередь публикации —
сразу, без паузы, до того как открыт WebSocket-порт. Payload и ключ
идемпотентности те же, что у прерванной попытки, поэтому кабинет, успевший
принять версию, видит дубль, а не вторую версию. Если в комнате есть ещё и
журнальный хвост после этой базы, его публикует та же очередь сразу после
успешной базы — по общему правилу «комната грязная, участников нет»: коммит
хвоста и публикация следующей версии, по порядку. Одна нечитаемая комната пропускается (о ней
скажет её `POST /v1/sessions`), старт из-за неё не падает. Флаг `conflict`
по-прежнему в памяти: после рестарта он обнаруживается повторной публикацией
неопубликованной базы, и `409` снова его ставит.

## 4. Жизненный цикл `conflict`

1. Коммит пишет базу локально и шлёт callback.
2. Кабинет отвечает `409` (файл изменили вне сессии).
3. Комната помечается `conflict`: правки продолжают приниматься и
   журналироваться, коммиты и серверные компакции — нет; очередь публикаций
   останавливается, потому что повторять тот же `expectedVersion` бессмысленно.
4. Оператор кабинета решает, что делать («новая сессия поверх нашей версии» или
   «сохранить копию»), и для первого варианта вызывает
   `POST /v1/sessions/{fileId}/resolve` с текущей версией и etag кабинета.
5. Resolve сразу публикует отклонённую базу с этой версией; ответ кабинета
   (`{version}`) становится ожиданием следующего коммита, т.е. разрешение
   действует до конца сессии, а не на один коммит.


## 5. Health и метрики

`GET /health`:

```json
{"ok":true,"rooms":2,"clients":3,"poisoned":[],"conflict":[]}
```

`ok:false` — журнал комнаты не записался (комната read-only, запись отклоняется,
commit разрешён как спасение состояния). `conflict` — комнаты, которым нужен
resolve из §2.2.

`GET /metrics` (Prometheus text):

| Метрика | Смысл |
|---|---|
| `pdfcraft_collab_rooms`, `pdfcraft_collab_clients` | комнаты и соединения |
| `pdfcraft_collab_room_version{room="…"}`, `_epoch` | голова комнаты |
| `pdfcraft_collab_room_read_only{room="…"}` | журнал не пишется |
| `pdfcraft_collab_room_conflict{room="…"}` | нужен resolve |
| `pdfcraft_collab_forced_commits_total` | серверные компакции журнала (порог операций, байт или простоя) |
| `pdfcraft_collab_publication_retries_total` | фоновые повторы callback (первая попытка восстановленной публикации не считается) |
| `pdfcraft_collab_publications_restored_total` | публикации, восстановленные из store при старте (§3.1) |
| `pdfcraft_collab_publications_after_resolve_total` | публикации, поставленные в очередь успешным resolve (§2.2) |
| `pdfcraft_collab_conflicts_resolved_total` | успешные resolve (§2.2) |

Флаг снимается сразу после resolve: `conflict` в `/health` пуст, строки
`pdfcraft_collab_room_conflict` в `/metrics` больше нет.

## 6. Состояние на диске

`--data/<fileId>/`:

- `CURRENT` — `{"version","epoch"}` последней записанной базы;
- `versions/<N>/file.pdf` — база; базы старше `CURRENT` удаляются на коммите;
- `journal.jsonl` — принятые операции после базы (пишутся до подтверждения);
  запись несёт `ms` — время принятия операции, по нему после рестарта
  восстанавливается idle-порог компакции (RFC §4.6);
- `CABINET` — `{"version","etag","expected"}`: `version` — версия комнаты у
  опубликованной базы (проверка «есть несохранённое» после рестарта),
  `expected` — номер той же копии в нумерации кабинета (следующий
  `expectedVersion`). До первого resolve они совпадают; маркер без `expected`
  читается как `expected = version`. `CURRENT.version > CABINET.version`
  означает, что база записана, но кабинет её не принял: такую публикацию
  восстанавливает старт демона (§3.1).

Удаление каталога сбрасывает комнату. `--documents` (dev-режим) сравнивает
etag файла с маркером перед коммитом: подмена файла — это `409`, как в проде.

## 7. Какие операции принимает комната

Действия и их схемы — RFC §3.1; здесь то, что видно эксплуатации. Класс
`merge` (комментарии и поля) проверяется только на роль (`commenter` и выше) и
эпоху; `structural` (страницы) дополнительно требует lease и головной версии.

| Действие | Класс | Адрес | Отказ без изменений |
|---|---|---|---|
| `annotation.add`, `annotation.reply`, `annotation.restore` | merge | страница или `/NM` родителя | `invalid`, `conflict` (родителя нет), `unsupported` (фигура вне §3.1.1), `limit` |
| `annotation.set_contents` | merge | `/NM` | `conflict` (текст изменился и это undo), `invalid` |
| `annotation.update` | merge | `/NM` | `conflict` (аннотации нет / undo перезаписал бы чужой вид), `invalid` (нет такого свойства, неверные `endings`), `unsupported` (залочена, вид не перерисовывается) |
| `annotation.font_size` | merge | `/NM` | `conflict` (undo перезаписал бы чужой размер), `invalid` (размер не 1–400, у комментария нет `/DA`), `unsupported` (залочен) |
| `annotation.info` | merge | `/NM` | `conflict` (undo перезаписал бы чужие свойства), `invalid` (пустое имя, неизвестная иконка, иконка не у заметки), `unsupported` (залочен) |
| `annotation.delete` | merge | `/NM` | `conflict` (аннотации нет) |
| `field.set_value` | merge | имя поля | `conflict` (undo), `invalid` |
| `page.*` | structural | страница (`page` + `epoch`) | `stale` (чужая эпоха — retryable), `lease` (retryable), `limit` (копия страницы больше 160 КиБ), `invalid` |
| `page.replace` | structural | страница (`page` + `epoch`) | `stale`, `lease`, `limit` (страница сверх 160 КиБ), `invalid` (битая base64, не ровно одна страница, страницы нет), `conflict` (undo, у которого нет копии) |
| `page.insert_blank` | structural | позиция вставки (`page` + `epoch`, `index` ≤ числа страниц) | `stale`, `lease`, `invalid` (размер не положительный/не конечный/больше 14400 pt) |
| `outline.item.add` | structural | ключ родителя (`outline_item.key`, `null` — верхний уровень) | `invalid` (родителя нет, пустой заголовок, страницы или имени нет), `limit` (заголовок > 512 байт, глубина > 32, закладок > 10 000) |
| `outline.item.update` | structural | ключ закладки | `invalid` (закладки нет, нечего менять, заголовок пуст или длинный), `limit` |
| `outline.item.delete`, `outline.item.move` | structural | ключ закладки | `invalid` (закладки нет, цикл, у `move` родителя нет), `limit` (поддерево > 2 000 узлов или > 128 КиБ) |
| `outline.item.restore` | structural | ключ родителя | `invalid` (копия не разбирается, нечего возвращать), `limit` (глубина, количество, размер копии) |
| `dest.add` | structural | имя (`named_dest.name`) | `conflict` (имя занято), `invalid` (страницы нет, имя имени, «никуда»), `limit` (> 10 000 имён), `stale` (payload `epoch` не текущий) |
| `dest.update`, `dest.delete` | structural | имя | `conflict` (имени нет), `invalid`, `limit`, `stale` |

Отказ не двигает `version`, не пишет строку журнала и не рассылается другим
клиентам: ответ `rejected` получает только отправитель, принятая операция
(`applied.op`) — все, включая автора. `retryable: true` (`stale`, `lease`,
`busy`) клиент повторяет после `resync`, `false` — показывает причину и не
применяет правку локально.

Закладки и named destinations — тоже `structural` (RFC §3.1.7): адрес у
закладки — объектная ссылка, у имени — само имя, а путь по ключу сервер
пересчитывает по своему документу перед каждой операцией. Удаление закладки
несёт в принятой операции всё поддерево, поэтому undo — обычный
`outline.item.restore`, а не серверная копия: `limit` на копию (2 000 узлов,
128 КиБ) срабатывает **до** удаления, и тогда закладка остаётся на месте.
`viewer` и `commenter` получают `forbidden` на любой из этих операций до
адаптера, поэтому ни `version`, ни `epoch`, ни документ не двигаются.

Иконка заметки и автор/subject перерисовывают её внешний вид на сервере
(`Edit::SetAnnotationInfo`), поэтому репликам не нужно считать ничего самим:
они применяют ту же операцию и получают тот же вид (RFC §3.1.3).
