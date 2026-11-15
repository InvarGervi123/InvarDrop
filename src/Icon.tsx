type Name = "device" | "shield" | "send" | "folder" | "alert" | "check";
const paths: Record<Name, string> = {
  device: "M5 3h14v12H5z M9 19h6 M12 15v4",
  shield: "M12 3 4 6v6c0 4 8 9 8 9s8-5 8-9V6z M8 12l3 3 5-6",
  send: "M12 20V4 M6 10l6-6 6 6",
  folder: "M3 7V5h6l2 2h10v13H3z",
  alert: "M12 3 2 21h20z M12 9v5 M12 17v1",
  check: "M4 12l5 5L20 6",
};
export function Icon({ name }: { name: Name }) {
  return <svg className="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false"><path d={paths[name]} /></svg>;
}
