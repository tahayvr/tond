// Human-readable cron schedules ("0 3 * * 1" → "At 03:00, on Monday").
//
// A small, dependency-free describer for the 5-field expressions (and the
// @hourly/@daily/… macros) the server accepts. Anything it can't phrase
// nicely falls back to a field-by-field description rather than failing.

const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const MONTHS = [
	'January',
	'February',
	'March',
	'April',
	'May',
	'June',
	'July',
	'August',
	'September',
	'October',
	'November',
	'December'
];
const DAY_NAMES: Record<string, number> = {
	sun: 0,
	mon: 1,
	tue: 2,
	wed: 3,
	thu: 4,
	fri: 5,
	sat: 6
};
const MONTH_NAMES: Record<string, number> = {
	jan: 1,
	feb: 2,
	mar: 3,
	apr: 4,
	may: 5,
	jun: 6,
	jul: 7,
	aug: 8,
	sep: 9,
	oct: 10,
	nov: 11,
	dec: 12
};

const MACROS: Record<string, string> = {
	'@yearly': '0 0 1 1 *',
	'@annually': '0 0 1 1 *',
	'@monthly': '0 0 1 * *',
	'@weekly': '0 0 * * 0',
	'@daily': '0 0 * * *',
	'@midnight': '0 0 * * *',
	'@hourly': '0 * * * *'
};

export interface CronDescription {
	ok: boolean;
	text: string;
}

const pad = (n: number) => String(n).padStart(2, '0');

function ordinal(n: number): string {
	const s = ['th', 'st', 'nd', 'rd'];
	const v = n % 100;
	return n + (s[(v - 20) % 10] || s[v] || s[0]);
}

function joinList(items: string[]): string {
	if (items.length <= 1) return items.join('');
	return `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`;
}

function normalize(field: string, names: Record<string, number>): string {
	return field.toLowerCase().replace(/[a-z]{3}/g, (m) => (m in names ? String(names[m]) : m));
}

const FIELD_RE = /^(\*|\d+(-\d+)?)(\/\d+)?(,(\*|\d+(-\d+)?)(\/\d+)?)*$/;

function validField(field: string, min: number, max: number): boolean {
	if (!FIELD_RE.test(field)) return false;
	return field.split(',').every((part) => {
		const [range, step] = part.split('/');
		if (step !== undefined && Number(step) < 1) return false;
		if (range === '*') return true;
		const [a, b] = range.split('-').map(Number);
		if (a < min || a > max) return false;
		if (b !== undefined && (b < min || b > max || b < a)) return false;
		return true;
	});
}

function describeList(field: string, render: (n: number) => string): string {
	return joinList(
		field.split(',').map((part) => {
			const [range, step] = part.split('/');
			if (range === '*' && step) return `every ${step}`;
			if (range.includes('-')) {
				const [a, b] = range.split('-').map(Number);
				return `${render(a)} through ${render(b)}${step ? ` (every ${step})` : ''}`;
			}
			return render(Number(range));
		})
	);
}

/** Describe a cron expression in plain English. `ok` is false for malformed input. */
export function describeCron(expression: string): CronDescription {
	const trimmed = expression.trim().replace(/\s+/g, ' ');
	if (!trimmed) return { ok: false, text: 'Enter a schedule' };
	const expanded = MACROS[trimmed.toLowerCase()] ?? trimmed;
	const fields = expanded.split(' ');
	if (fields.length !== 5) {
		return {
			ok: false,
			text: 'A schedule needs 5 fields: minute hour day-of-month month day-of-week'
		};
	}
	const [minute, hour, dom, monthRaw, dowRaw] = fields;
	const month = normalize(monthRaw, MONTH_NAMES);
	const dow = normalize(dowRaw, DAY_NAMES).replace(/\b7\b/g, '0');

	const checks: [string, string, number, number][] = [
		['minute', minute, 0, 59],
		['hour', hour, 0, 23],
		['day of month', dom, 1, 31],
		['month', month, 1, 12],
		['day of week', dow, 0, 6]
	];
	for (const [name, value, min, max] of checks) {
		if (!validField(value, min, max)) {
			return { ok: false, text: `Invalid ${name} field “${value}” (allowed ${min}–${max})` };
		}
	}

	const simple = (f: string) => /^\d+$/.test(f);
	let time: string;
	if (simple(minute) && simple(hour)) {
		time = `At ${pad(Number(hour))}:${pad(Number(minute))}`;
	} else if (minute === '*' && hour === '*') {
		time = 'Every minute';
	} else if (/^\*\/\d+$/.test(minute) && hour === '*') {
		time = `Every ${minute.slice(2)} minutes`;
	} else if (simple(minute) && hour === '*') {
		time = Number(minute) === 0 ? 'Every hour' : `At ${Number(minute)} minutes past every hour`;
	} else if (simple(minute) && /^\*\/\d+$/.test(hour)) {
		time = `At ${Number(minute)} minutes past every ${hour.slice(2)} hours`;
	} else if (simple(minute) && /^[\d,]+$/.test(hour)) {
		time = `At ${joinList(hour.split(',').map((h) => `${pad(Number(h))}:${pad(Number(minute))}`))}`;
	} else {
		const minutePart = minute === '*' ? 'every minute' : `minute ${describeList(minute, String)}`;
		const hourPart = hour === '*' ? '' : ` past hour ${describeList(hour, String)}`;
		time = `At ${minutePart}${hourPart}`;
	}

	const parts = [time];
	if (dom !== '*') parts.push(`on the ${describeList(dom, ordinal)}`);
	if (month !== '*') parts.push(`in ${describeList(month, (m) => MONTHS[m - 1])}`);
	if (dow !== '*') {
		const days = dow === '1-5' ? 'weekdays' : describeList(dow, (d) => DAYS[d]);
		parts.push(`${dom !== '*' ? 'and on' : 'on'} ${days}`);
	}
	return { ok: true, text: parts.join(', ') };
}

/** All IANA time zones the browser knows, with UTC first. */
export function timeZones(): string[] {
	let zones: string[];
	try {
		zones =
			(Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.(
				'timeZone'
			) ?? [];
	} catch {
		zones = [];
	}
	const unique = new Set(['UTC', ...zones]);
	return [...unique];
}

/** The viewer's own time zone, e.g. "Europe/Berlin". */
export function browserTimeZone(): string {
	try {
		return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
	} catch {
		return 'UTC';
	}
}
