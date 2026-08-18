const POOL_URL_PATTERN = /^cmfd\+tls:\/\/(\[[0-9A-Fa-f:.]+\]|(?:\d{1,3}\.){3}\d{1,3}):([0-9]{1,5})\?pin=([0-9A-Fa-f]{64})$/;
const WORKER_NAME_PATTERN = /^[A-Za-z0-9._-]{1,32}$/;

export const POOL_URL_REQUIREMENT =
  "Use cmfd+tls://PRIVATE_IP:PORT?pin=64_HEX with a private or loopback numeric IP.";
export const WORKER_NAME_REQUIREMENT =
  "Use 1–32 letters, numbers, dots, underscores, or hyphens.";

function isPrivateOrLoopbackIpv4(host: string): boolean {
  const octets = host.split(".");
  if (octets.length !== 4) return false;
  if (octets.some((octet) => (
    !/^\d{1,3}$/.test(octet)
    || (octet.length > 1 && octet.startsWith("0"))
    || Number(octet) > 255
  ))) return false;

  const [first, second] = octets.map(Number);
  return first === 10
    || first === 127
    || (first === 172 && second >= 16 && second <= 31)
    || (first === 192 && second === 168);
}

function isPrivateOrLoopbackIpv6(host: string): boolean {
  const normalized = host.toLowerCase();
  if (normalized === "::1" || normalized === "0:0:0:0:0:0:0:1") return true;

  const firstHextet = Number.parseInt(normalized.split(":", 1)[0], 16);
  return firstHextet >= 0xfc00 && firstHextet <= 0xfdff;
}

export function poolUrlError(value: string): string | null {
  const match = POOL_URL_PATTERN.exec(value);
  if (!match) return POOL_URL_REQUIREMENT;

  const [, rawHost, rawPort] = match;
  const port = Number(rawPort);
  if (!Number.isInteger(port) || port < 1 || port > 65_535) {
    return POOL_URL_REQUIREMENT;
  }

  if (rawHost.startsWith("[")) {
    try {
      new URL(value);
    } catch {
      return POOL_URL_REQUIREMENT;
    }
    const host = rawHost.slice(1, -1);
    return isPrivateOrLoopbackIpv6(host) ? null : POOL_URL_REQUIREMENT;
  }

  return isPrivateOrLoopbackIpv4(rawHost) ? null : POOL_URL_REQUIREMENT;
}

export function workerNameError(value: string): string | null {
  return WORKER_NAME_PATTERN.test(value) ? null : WORKER_NAME_REQUIREMENT;
}
