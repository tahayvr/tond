// Short relative times for cards and lists ("5m ago", "in 2h").

export function formatRelativePast(dt: string | null | undefined, now = Date.now()): string {
	if (!dt) return 'never';
	const diff = now - new Date(dt).getTime();
	if (Number.isNaN(diff)) return '—';
	const minutes = Math.floor(diff / 60_000);
	if (minutes < 1) return 'just now';
	if (minutes < 60) return `${minutes}m ago`;
	const hours = Math.floor(minutes / 60);
	if (hours < 24) return `${hours}h ago`;
	return `${Math.floor(hours / 24)}d ago`;
}

export function formatRelativeFuture(dt: string | null | undefined, now = Date.now()): string {
	if (!dt) return '—';
	const diff = new Date(dt).getTime() - now;
	if (Number.isNaN(diff)) return '—';
	if (diff < 0) return 'due now';
	const minutes = Math.floor(diff / 60_000);
	if (minutes < 1) return 'in <1m';
	if (minutes < 60) return `in ${minutes}m`;
	const hours = Math.floor(minutes / 60);
	if (hours < 24) return `in ${hours}h`;
	return `in ${Math.floor(hours / 24)}d`;
}
