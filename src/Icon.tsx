type Name = "device" | "phone" | "shield" | "send" | "folder" | "alert" | "check"
  | "settings" | "file" | "add" | "close" | "back" | "inbox" | "nearby" | "brand";
const paths: Record<Name, string> = {
  brand: "M12 3v12 M7.5 10.5 12 15l4.5-4.5 M4 15v5h16v-5",
  phone: "M8 2.75h8a2 2 0 0 1 2 2v14.5a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V4.75a2 2 0 0 1 2-2z M11 18h2",
  settings: "m10 3-.6 2.3-1.7 1-2.3-.7L3.4 9l1.7 1.7v2.6L3.4 15l2 3.4 2.3-.7 1.7 1 .6 2.3h4l.6-2.3 1.7-1 2.3.7 2-3.4-1.7-1.7v-2.6L20.6 9l-2-3.4-2.3.7-1.7-1L14 3z M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
  file: "M6 3h8l4 4v14H6z M14 3v5h4 M9 12h6 M9 16h6",
  add: "M12 5v14 M5 12h14",
  close: "m6 6 12 12 M18 6 6 18",
  back: "m10 5-7 7 7 7 M3 12h17",
  inbox: "M5 4h14l3 10v6H2v-6z M2 14h6l2 2h4l2-2h6",
  nearby: "M3.5 8a12 12 0 0 1 17 0 M6.5 11a8 8 0 0 1 11 0 M9.5 14a3.5 3.5 0 0 1 5 0 M12 17.5v.5",
  device: "M5 3h14v12H5z M9 19h6 M12 15v4",
  shield: "M12 3 4 6v6c0 4 8 9 8 9s8-5 8-9V6z M8 12l3 3 5-6",
  send: "m3 11 18-7-7 16-3-7z M11 13 21 4",
  folder: "M3 7V5h6l2 2h10v13H3z",
  alert: "M12 3 2 21h20z M12 9v5 M12 17v1",
  check: "M4 12l5 5L20 6",
};
export function Icon({ name }: { name: Name }) {
  return <svg className="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false"><path d={paths[name]} /></svg>;
}
