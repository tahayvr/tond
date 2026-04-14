// Logos for supported database engines.
// Each entry maps a db_type string (must match ALL_ENGINES in tond-common) to
// a public image URL.  Local assets are imported with Vite's `?url` suffix so
// they are fingerprinted and served correctly; remote URLs are used as-is.

import keydbLogo from './brand-logo-assets/keydb.svg?url';

export const DB_LOGOS: Record<string, string> = {
	postgres:
		'https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/postgresql/postgresql-original.svg',
	mysql: 'https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/mysql/mysql-original.svg',
	mariadb: 'https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/mariadb/mariadb-original.svg',
	redis: 'https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/redis/redis-original.svg',
	keydb: keydbLogo,
	mongodb: 'https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/mongodb/mongodb-original.svg'
};

/** Returns the logo URL for a given db_type, or null if none is registered. */
export function dbLogoUrl(dbType: string): string | null {
	return DB_LOGOS[dbType] ?? null;
}
