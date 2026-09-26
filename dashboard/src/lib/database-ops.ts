// Helpers for the managed-database essentials: backup schedules, restore into
// a new database, versions and variants, the public-access allowlist.

import type { DbCatalogEntry, ManagedDatabase } from '$lib/api';
import { describeCron } from '$lib/cron-schedule';

/** Backup schedule presets (UTC). `custom` means "type a cron expression". */
export const BACKUP_PRESETS: { value: string; label: string }[] = [
	{ value: '0 3 * * *', label: 'Every day at 03:00' },
	{ value: '0 */6 * * *', label: 'Every 6 hours' },
	{ value: '0 * * * *', label: 'Every hour' },
	{ value: '0 3 * * 0', label: 'Every Sunday at 03:00' }
];

/** Which preset a schedule matches: a preset value, "off" or "custom". */
export function presetFor(schedule: string | null): string {
	if (!schedule) return 'off';
	return BACKUP_PRESETS.some((p) => p.value === schedule) ? schedule : 'custom';
}

/** Plain-English schedule ("Every day at 03:00 UTC"), or null when off. */
export function describeBackupSchedule(schedule: string | null): string | null {
	if (!schedule) return null;
	const preset = BACKUP_PRESETS.find((p) => p.value === schedule);
	if (preset) return `${preset.label} UTC`;
	const d = describeCron(schedule);
	return d.ok ? `${d.text} (UTC)` : schedule;
}

/** A free name for a restored copy: "<name>-restore-<yyyymmdd>", suffixed if taken. */
export function restoreName(source: string, date: Date, taken: string[] = []): string {
	const stamp = date.toISOString().slice(0, 10).replaceAll('-', '');
	const base = `${source}-restore-${stamp}`.slice(0, 60);
	if (!taken.includes(base)) return base;
	for (let i = 2; i < 100; i++) {
		const candidate = `${base}-${i}`;
		if (!taken.includes(candidate)) return candidate;
	}
	return `${base}-${Date.now()}`;
}

/** Database names: letters, digits and hyphens, not starting with a hyphen. */
export function validDatabaseName(name: string): boolean {
	return /^[A-Za-z0-9][A-Za-z0-9-]{0,62}$/.test(name);
}

/** "PostgreSQL 16 · pgvector" */
export function engineLabel(db: Pick<ManagedDatabase, 'db_type' | 'version' | 'variant'>): string {
	const names: Record<string, string> = {
		postgres: 'PostgreSQL',
		mysql: 'MySQL',
		mariadb: 'MariaDB',
		redis: 'Redis',
		keydb: 'KeyDB',
		mongodb: 'MongoDB'
	};
	const name = names[db.db_type] ?? db.db_type;
	const version = db.version && db.version !== 'latest' ? ` ${db.version}` : '';
	const variant = db.variant ? ` · ${db.variant === 'pgvector' ? 'pgvector' : 'PostGIS'}` : '';
	return `${name}${version}${variant}`;
}

/** Catalog entries (engine's own image first, then variants) for one engine. */
export function catalogFor(catalog: DbCatalogEntry[], dbType: string): DbCatalogEntry[] {
	return catalog
		.filter((e) => e.db_type === dbType)
		.sort((a, b) => (a.variant === '' ? -1 : b.variant === '' ? 1 : 0));
}

/** What an upgrade from one major version to another means, for the warning. */
export function upgradeWarnings(dbType: string, from: string, to: string): string[] {
	const out = [
		`Apps lose their connection to the database for the whole upgrade: Tond copies the data from version ${from} into a fresh version ${to} and switches over.`,
		'A backup is taken first. If anything fails before the switch, nothing changes.',
		`The version ${from} data is kept until you confirm, so you can roll back. Writes made after the upgrade are lost if you roll back.`,
		'Downgrades are not possible after you confirm.'
	];
	if (dbType === 'postgres') {
		out.push('Check that your app and its database driver support the new version first.');
	}
	return out;
}

/**
 * Parse allowlist text (one address or CIDR range per line, commas allowed).
 * Returns the entries and the first invalid one, if any. IPv4 only: public
 * databases are published on IPv4.
 */
export function parseAllowlist(text: string): { entries: string[]; invalid: string | null } {
	const entries: string[] = [];
	for (const raw of text.split(/[\s,]+/)) {
		const entry = raw.trim();
		if (!entry) continue;
		const [addr, prefix, extra] = entry.split('/');
		const octets = addr.split('.');
		const ipOk = octets.length === 4 && octets.every((o) => /^\d{1,3}$/.test(o) && Number(o) <= 255);
		const prefixOk =
			prefix === undefined || (/^\d{1,2}$/.test(prefix) && Number(prefix) >= 1 && Number(prefix) <= 32);
		if (!ipOk || !prefixOk || extra !== undefined) return { entries, invalid: entry };
		if (!entries.includes(entry)) entries.push(entry);
	}
	return { entries, invalid: null };
}
