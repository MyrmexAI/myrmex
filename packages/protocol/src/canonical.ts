// Канонический JSON: ключи по алфавиту, без пробелов, только целые числа, без undefined.
// Канонический формат: от него зависят все хеши и подписи, менять нельзя.

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

export class CanonicalJsonError extends Error {}

export function canonicalJson(value: unknown): string {
  if (value === null) return 'null';
  switch (typeof value) {
    case 'boolean':
      return value ? 'true' : 'false';
    case 'string':
      return JSON.stringify(value);
    case 'number':
      if (!Number.isSafeInteger(value)) {
        throw new CanonicalJsonError(`только безопасные целые числа, получено ${value}`);
      }
      return String(value);
    case 'object': {
      if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
      const proto = Object.getPrototypeOf(value);
      if (proto !== Object.prototype && proto !== null) {
        throw new CanonicalJsonError('только простые объекты');
      }
      const obj = value as Record<string, unknown>;
      const keys = Object.keys(obj).sort();
      const parts: string[] = [];
      for (const k of keys) {
        if (obj[k] === undefined) throw new CanonicalJsonError(`undefined в поле ${k}`);
        parts.push(`${JSON.stringify(k)}:${canonicalJson(obj[k])}`);
      }
      return `{${parts.join(',')}}`;
    }
    default:
      throw new CanonicalJsonError(`неподдерживаемый тип ${typeof value}`);
  }
}
