# Milestone 2.2 — TLS и pairing

Документ фиксирует протокол и результаты Milestone 2.2, объединённого в `main`
через PR #6. В Milestone 2.3 сохранены identity/trust и правила pairing, но транспорт
использует ALPN `invardrop/2`, hello version 2 и новые файловые сообщения.
Актуальные дополнения и границы передачи описаны в [transfers.md](transfers.md).

## Область и модель угроз

Цель — установить проверенное зашифрованное соединение между экземплярами
InvarDrop без аккаунта, облачного сервера и публичного CA. Передачи файлов нет.

Предполагается, что противник в LAN может подделывать mDNS, адреса и имена,
поднимать собственные TLS endpoints, перехватывать/перенаправлять соединения,
отправлять malformed messages, повторять запросы и отключать сеть. Он не должен
получить доверие только потому, что его имя похоже на известное устройство.

Доверенными считаются ОС, целостность приложения/его WebView, криптографические
библиотеки и пользователь, сравнивающий **все** группы кода на двух нужных
устройствах через независимый канал. Компрометация текущей учётной записи,
администратора, Secret Service/Credential Manager или подмена GUI вне модели.
Любой процесс того же пользователя может иметь доступ к keyring; это не HSM.
Проект не проходил независимый аудит или формальную проверку протокола.

## Криптографическая идентичность

- Для профиля генерируется ECDSA P-256 key pair (`rcgen`/`ring`).
- Самоподписанный X.509 certificate — контейнер публичного ключа, а не CA identity.
- Trusted identity = SHA-256 от DER SubjectPublicKeyInfo (SPKI), 256 бит.
- Private key и certificate сохраняются; временный mDNS UUID не используется
  как криптографическая идентичность. Постоянный fingerprint не публикуется в mDNS.
- При чтении проверяется соответствие private key и certificate. Повреждённая
  или отсутствующая часть существующей идентичности не заменяется автоматически.

TLS использует stable rustls 0.23 с ring provider и tokio-rustls. Только TLS 1.3;
ALPN `invardrop/1` обязателен. TLS resumption, tickets и 0-RTT выключены, key logging
не включается. Обе стороны обязаны представить certificate и доказать владение
ключом через стандартную проверку TLS CertificateVerify signature в rustls.

Verifier намеренно не использует Web PKI, DNS hostname, certificate CN/SAN или
его срок действия как источник доверия. Для неизвестного ключа certificate
допускается **только в карантин pairing** после ASN.1/SPKI проверки; TLS signatures
никогда не пропускаются. Для явно заданного known peer pin другой SPKI отклоняется
уже при TLS-handshake, без fallback к новому pairing. Собственный ключ отвергается.
Изменение certificate при сохранённом SPKI не меняет identity; ротация самого ключа
требует нового явного pairing. Политика основана на ключах, а не certificate expiry.

## Код сравнения и согласие

После TLS обе стороны получают 16 байт через TLS exporter с label
`EXPORTER-InvarDrop-Pairing-v1` и context фиксированного формата:
`client:<client-SPKI-hash>;server:<server-SPKI-hash>`.

Код = все 128 бит exporter output, показанные как 8 групп по 4 hex-символа.
Имена, mDNS UUID и объявления в вычислении не участвуют. Код никогда не передаётся
peer как application message и не пишется в логи. Это проверяемое человеком
связывание двух экранов с одним TLS-каналом и обеими доказанными ключевыми парами.
Сам факт завершения TLS с неизвестным ключом **не аутентифицирует нужное устройство**.

Первое соединение:

1. TCP → mutual TLS 1.3 → проверка ALPN, ключей, self/revocation.
2. Внутри TLS обмениваются `hello(version=1, initiator, trusted)`.
3. Если ключи не доверены с обеих сторон, UI показывает полный код. На **каждом**
   устройстве пользователь сравнивает код целиком, отмечает checkbox и нажимает
   `Codes match — pair`. Нет auto-focus/Enter default, TOFU или авто-подтверждения.
4. По TLS передаётся только `decision(approve=true/false)`. Backend принимает
   локальное подтверждение только для текущего connection ID, совпадающего кода,
   корректной фазы и неистёкшего срока. Дубликаты/старые approvals отклоняются.
5. Только после обоих approvals возможна фаза `committing`, скрытие кода и запись доверия.
   Затем обе стороны отправляют `ready`; лишь после remote ready UI показывает
   `Secure connection`.

Reject до локального подтверждения отменяет сессию. После отправленного согласия
Cancel закрывает соединение, но **не является отзывом ранее сохранённого доверия**.
Для отзыва есть Revoke trust. Отмена до commit не должна создать локальное доверие;
блокирующая файловая операция после полученных согласий может завершиться даже
при сетевой отмене. Эти границы не следует скрывать за словом «атомарно».
Если запись завершилась после отмены соединения, задача сохранения всё равно
обновляет список доверия в UI: сохранённый ключ остаётся виден для Revoke trust.

При reconnect обе стороны должны знать соответствующие SPKI; код и новое
подтверждение не нужны, владение ключами заново проверяется в TLS. Если состояние
доверия асимметрично, соединение закрывается, без тихого перехода к pairing.
Чтобы переподключиться после ошибки частичного commit, следует явно удалить/
отозвать старое доверие на обеих сторонах, разрешить новое pairing и сравнить код
заново. Глобальной транзакции между двумя устройствами нет.

Аббревиатура fingerprint в UI служит навигации, **не заменяет полный pairing-код**.
Тест с двумя TLS-каналами MITM подтверждает разные коды в этом сценарии; это не
формальное доказательство отсутствия всех MITM-атак. Если человек не сравнивает
весь код на правильных устройствах, требуемая аутентификация не достигается.

## Хранение и отзыв

- ОС keyring хранит случайный 32-байтный master key: Linux Secret Service,
  Windows Credential Manager (`keyring` v4). Нет fallback в plaintext или временную
  identity. Если keyring недоступен/заблокирован, secure listener не запускается.
- `identity.enc` содержит versioned JSON identity/trust/revocation state,
  защищённый ChaCha20-Poly1305 (`ring`), с новым случайным 96-битным nonce на запись
  и version header как authenticated associated data. Crypto primitives не свои.
- Обновление: tempfile, `sync_all`, atomic replace, directory sync на Unix.
  Unix profile directory имеет mode 0700, tempfile — 0600. На Windows используются
  наследуемые ACL пользовательского app-data каталога; содержимое дополнительно AEAD.
- Exclusive OS file lock запрещает два writer-процесса одного профиля.
- Есть ограничения 64 trusted / 256 revoked identities и 64 KiB файла до парсинга.
- Revoke удаляет trust, сохраняет revoked marker и закрывает активные сессии.
  Revoked key нельзя принять автоматически или повторно доверить без Unblock.
  `Allow new pairing` лишь удаляет блокировку, не восстанавливает trust.
- Ошибка сохранения нового trust не авторизует peer в памяти. При ошибке записи
  revoke доверие запрещается в текущем процессе, соединения закрываются и UI
  получает ошибку. **Такой отзыв не считается долговечным**: после перезапуска
  на диске может остаться старое состояние. Нужно устранить ошибку и повторить отзыв.
- AEAD не обнаруживает rollback старой корректно зашифрованной резервной копии.
  Файловый rollback/local-account compromise не входит в гарантии этого этапа.

Профиль по умолчанию — `security-v1` в пользовательском app-data Tauri. Для
независимых нативных тестов можно задать абсолютный `INVARDROP_PROFILE`.
Для каждого такого профиля создаётся отдельная запись keyring, привязанная к hash
его canonical path. Не переносите/удаляйте профиль и credential независимо: при
потере одной части приложение должно отказать, а не сменить ключ незаметно.
Штатного export/recovery/rotation UI на этом этапе нет.

## Discovery, протокол и ресурсы

Discovery v1 (`mode=discovery`, port 0) по-прежнему распознаётся, но Connect
выключен. Готовый secure listener объявляет v2, `mode=secure` и свой реальный TCP
порт. Поля маршрутизации не аутентифицированы. IP/port не передаются в React:
команда Connect принимает discovery ID, Rust получает текущие endpoints из tracker.
При pinned reconnect endpoint проверяется ожидаемым ключом, не именем/IP.

Dial ограничен private/link-local/loopback IPv4 и ULA/link-local/loopback IPv6;
public/global IPv6 endpoints не используются. Это осознанное ограничение LAN
маршрутизации, не криптографическая аутентификация адреса. Listener предпочитает
IPv6 dual-stack с IPv4 fallback. IPv6-only/global-prefix LAN, VPN, firewall и
несколько физических устройств требуют отдельных испытаний.

Application frames существуют **только внутри TLS**: 4-байтная big-endian длина,
затем JSON до 2048 байт. `hello`, `decision`, `ready`, `ping`, `pong` — единственные
типы; лишние поля, неожиданные фазы, неверная версия/роли и oversized frames
отклоняются. File/message transfer API отсутствует.

Лимиты: 8 одновременных сессий, 30 попыток/минуту, backlog 64, до 8 адресов на
попытку, TCP dial 500 ms/адрес в общем TLS deadline 5 s, hello/ready deadline 5 s,
pairing 90 s, idle timeout 20 s, keepalive 5 s, до 64 полученных keepalive frames/
минуту/сессию. Очередь входящих frames — 4, UI actions — 2; история — 24 записи.
TCP-reader выделен в async task, чтобы отмена ожидания UI не теряла partial frame.
Отмена и shutdown закрывают соединения, вызывают TLS shutdown там, где TLS уже
установлен, и завершают задачи. Ожидание ОС keyring — блокирующая операция вне
GUI thread: её нельзя считать мгновенно отменяемой; listener до неё не открывается.

Это ограничивает ресурсы приложения, но не гарантирует доступность на hostile
LAN: злоумышленник может занять слоты, глушить multicast или обрывать TCP.

## Проверки и воспроизведение

```sh
npm ci
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --no-bundle -- --locked
```

Rust tests используют настоящие TCP/TLS sockets, crypto и AEAD files, но master
keys тестового storage держат в памяти вместо системного keyring. Это не fallback
production-кода. Тестируются matching keys/code, обе approvals, rejection,
expiration, malformed/oversized messages, wrong pin, два разных MITM-канала,
reconnect, restart с сохранёнными ключами, revoke, corruption, write failures,
exclusive profile и завершение stalled handshakes. Нативное keyring/UI поведение
нужно проверять отдельно.

Нативный сценарий: открыть два профиля, Connect, убедиться в отсутствии
авто-подтверждения; сравнить весь код на обоих экранах; подтвердить на обоих;
проверить Secure connection, Disconnect/Reconnect, перезапуск профилей,
Revoke trust и отказ повторного соединения. Отдельно проверить Reject и Cancel.
Не выводить pairing-коды, private keys и TLS exporter в test logs.

## Фактическая регрессия — 2026-11-13

Локально: CachyOS x86_64, Niri/Wayland, Linux 7.3.3, Rust 1.99.1,
Node.js 26.13.1, npm 12.2.2; CI использует Node.js 24.

- Полный `cargo test --locked`: 40 passed, 0 failed, без ignored tests.
  Реальные TCP/TLS tests требуют разрешённых сокетов и запускались вне sandbox.
- Rustfmt, Clippy `--all-targets -- -D warnings`, обе TypeScript-проверки,
  Vite production build и locked Tauri release build без bundle прошли.
- Сохранены и завершены регрессии asymmetric trust и partial TCP frame во время
  локального approval. Добавлены проверки повторного/устаревшего approval,
  отмены после одностороннего согласия, self-identity, revoke во время pairing,
  свежего nonce, malformed/oversized storage и ошибок revoke/unblock.
- Исправлена гонка отмены во время commit: сохранённый ключ мог отсутствовать
  в snapshot UI. Детерминированный тест блокирует store на время отмены, затем
  проверяет публикацию завершённого commit и возможность отзыва.
- Чтение trust/revocation и публикация snapshot сериализованы: параллельный refresh
  больше не может перезаписать новый отзыв старым списком с новой revision.
  Регрессия с управляемым расписанием потоков воспроизводит ошибку в старом коде
  и проходит после исправления; внешний notifier вызывается без удержания mutex.
- Два независимых production GUI-процесса с отдельными `INVARDROP_PROFILE`
  использовали настоящий Linux Secret Service, реальные mDNS и TCP/TLS.
  Через AT-SPI проверены совпадение всех восьми групп кода без вывода значений,
  выключенная кнопка approval до checkbox, Reject, Cancel после одного approval,
  оба approvals, Disconnect и pinned Reconnect.
- После штатного закрытия и повторного запуска обоих процессов проверены прежние
  public fingerprints и trust, подключение без нового кода, Revoke с закрытием
  соединения, отказ reconnect и Unblock без восстановления trust.

Для Windows проверено locked дерево зависимостей `x86_64-pc-windows-msvc`:
keyring выбирает Windows Credential Manager; Unix permissions/directory sync
защищены `cfg(unix)`. CI выполняет native build, строгий Clippy и tests на
`windows-2022` и `ubuntu-24.04`; актуальный результат относится к конкретному
commit в [GitHub Actions](https://github.com/InvarGervi123/InvarDrop/actions/workflows/ci.yml).
Локального Windows toolchain/runtime нет. CI не проверяет Windows GUI,
Credential Manager и multicast во время работы приложения.

Два физических устройства, Windows runtime, IPv6-only/global-prefix LAN,
Wi-Fi/VPN/firewall transitions, длительные потери пакетов, недоступный/заблокированный
системный keyring и независимый аудит остаются отдельными проверками.
Передачи файлов, export/recovery identity и транзакции trust между устройствами нет.
