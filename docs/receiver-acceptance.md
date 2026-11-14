# Milestone 2.4 — интерфейс согласия получателя

Расширен существующий UI 2.3; streaming, BLAKE3, фиксированный inbox и no-clobber
finalization не переписаны. Incoming request показывает имя отправителя, basename,
читаемый размер и состояния waiting / accepted / declined / expired / completed /
failed. Имя передано внутри проверенного TLS, но остаётся **самоназванием peer**:
его нельзя использовать вместо ключа. Контрольные и bidi-control символы, пустые
имена и имена длиннее 80 UTF-8 байт отклоняются до публикации запроса.

Имя приложения берётся из текущего local discovery name. Протокол явно обновлён
до ALPN `invardrop/3`, hello version 3, `file_offer.sender_name`; оба устройства
нужно обновить. Identity/trust 2.2–2.3 сохраняются, новое pairing не требуется.
Транспорт 2.3 отклоняется без fallback, что проверено отрицательным TLS-тестом.

Перед Accept нужен unchecked checkbox «I want to save this file». У кнопок
`type=button`, нет autofocus/формы/default Enter. Decline доступен без checkbox.
UI блокирует повторный вызов синхронным ref и сохраняет блокировку после успешного
IPC до нового snapshot. Checkbox привязан к transfer ID и не переносится на новый
запрос. При ошибке IPC можно повторить действие. Backend независимо проверяет
phase, transfer ID, направление, отсутствие предыдущего решения и deadline.

Без Accept Sink не создаётся. После Decline файл не сохраняется; после 90 s без
решения запрос истекает, сессия закрывается и UI получает `expired`. Данные до
Accept по-прежнему закрывают соединение. Все границы отмены/finalization и оговорки
из [transfers.md](transfers.md) сохранены. Состояние «Accepted» не означает успешной
передачи: завершение отображается только после проверки и сохранения.

## Проверки

`npm test` запускает Vitest/jsdom/Testing Library, также включён в обе платформы CI.
Проверяются реальные DOM-кнопки, checkbox, повторные действия, ошибка IPC, смена
request ID, readable size и все терминальные состояния. Rust регрессия проверяет
истечение настоящего TLS offer без файла, позднее согласие и отсутствие публикации
запроса с небезопасным sender name. Сохранены отрицательные проверки отказа,
повреждения, усечения, отключения, overwrite и replay из 2.3.

Для нативного GUI smoke test добавлен `receiver_probe` — диагностический sender
через обычные Service/Source API. Он находит только явно указанное discovery name,
принимает команды через stdin (`status`, `approve <receiver code>`, `send`, `quit`).
Код comparison передаётся тестовым драйвером через pipe без вывода в логи.
Это позволяет проверить настоящее окно receiver и Tauri IPC без автоматизации
GTK file picker; этот probe не подключён к production UI.

## Фактические результаты — 2026-11-14

Локально: 13 UI-тестов, 59 Rust-тестов, TypeScript, Rustfmt, строгий Clippy
`--all-targets -- -D warnings` и locked production build прошли.
Linux production GUI + диагностический sender с настоящим Secret Service,
реальным mDNS и TLS прошли pairing с проверкой кода без его логирования, отображение
имени и 1 MiB, блокировку Accept до checkbox, Decline без файла, повторный offer,
Accept через Tauri IPC, Completed/BLAKE3 и побайтовое сравнение результата.
Тест повторён в окне receiver 360×420; GUI exit 0. Изолированные тестовые credentials
удалены. Системные настройки темы не менялись: новые стили используют существующие
light/dark CSS variables, системный theme workflow 2.1 сохранён.

Это полноценный native receiver workflow с программным sender. Нативный picker
sender, два GUI одновременно на физических устройствах и Windows GUI/keyring
этой проверкой не покрыты. Неотменяемая finalization, ack loss и crash cleanup
имеют прежние ограничения 2.3. CI отдельно запускает UI/Rust tests, Clippy,
TypeScript, Rustfmt и production builds на Linux и Windows.
