const ATOMS_PER_CMFD = 100_000_000n;
const DISPLAY_UNITS_PER_CMFD = 100n;
const ATOMS_PER_DISPLAY_UNIT = ATOMS_PER_CMFD / DISPLAY_UNITS_PER_CMFD;
const CMFD_PATTERN = /^(?:0|[1-9]\d*)(?:\.(\d{1,8}))?$/;

export function formatAtoms(value: string | bigint, signed = false): string {
  const atoms = typeof value === "bigint" ? value : BigInt(value);
  const negative = atoms < 0n;
  const absolute = negative ? -atoms : atoms;
  const whole = absolute / ATOMS_PER_CMFD;
  const fraction = (absolute % ATOMS_PER_CMFD).toString().padStart(8, "0");
  const grouped = groupWhole(whole);
  const prefix = negative ? "−" : signed && atoms > 0n ? "+" : "";
  return `${prefix}${grouped}.${fraction}`;
}

export function formatBalanceAtoms(value: string | bigint): string {
  const atoms = typeof value === "bigint" ? value : BigInt(value);
  const negative = atoms < 0n;
  const absolute = negative ? -atoms : atoms;
  const displayUnits = (absolute + ATOMS_PER_DISPLAY_UNIT / 2n) / ATOMS_PER_DISPLAY_UNIT;
  const whole = displayUnits / DISPLAY_UNITS_PER_CMFD;
  const fraction = (displayUnits % DISPLAY_UNITS_PER_CMFD).toString().padStart(2, "0");
  const prefix = displayUnits === 0n ? "" : negative ? "−" : "";
  return `${prefix}${groupWhole(whole)}.${fraction}`;
}

function groupWhole(value: bigint) {
  return new Intl.NumberFormat("en-US", {
    maximumFractionDigits: 0,
    useGrouping: true,
  }).format(value);
}

export function parseCmfd(value: string): bigint | null {
  const normalized = value.trim();
  const match = CMFD_PATTERN.exec(normalized);
  if (!match) return null;
  const fraction = (match[1] ?? "").padEnd(8, "0");
  try {
    return BigInt(normalized.split(".")[0]) * ATOMS_PER_CMFD + BigInt(fraction || "0");
  } catch {
    return null;
  }
}

export function sumAtoms(...values: string[]): bigint {
  return values.reduce((total, value) => total + BigInt(value), 0n);
}

export function shortenHash(value: string, head = 8, tail = 8): string {
  if (value.length <= head + tail + 1) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}

export function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  return `${(value / 1024).toFixed(1)} KiB`;
}

export function formatTimestamp(value: number | null): string {
  if (value === null) return "Pending";
  return new Intl.DateTimeFormat("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(new Date(value * 1000));
}
