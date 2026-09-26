// Port and resource hints for the new-app wizard: read the ports a Dockerfile
// EXPOSEs or a compose file publishes, and explain the default memory cap.

/** Ports from `EXPOSE` lines (TCP only; "8080/udp" is skipped). */
export function parseDockerfileExpose(content: string): number[] {
	const ports: number[] = [];
	for (const raw of content.split('\n')) {
		const line = raw.trim();
		const m = /^expose\s+(.+)$/i.exec(line);
		if (!m) continue;
		for (const token of m[1].split(/\s+/)) {
			const [port, proto = 'tcp'] = token.split('/');
			const n = Number(port);
			if (proto.toLowerCase() === 'tcp' && Number.isInteger(n) && n > 0 && n < 65536) {
				ports.push(n);
			}
		}
	}
	return [...new Set(ports)];
}

/** Container port of one short-syntax compose port entry ("8080:80", "127.0.0.1:8080:80/tcp", "80"). */
function containerPortOf(entry: string): number | null {
	const cleaned = entry.trim().replace(/^["']|["']$/g, '');
	const [spec, proto = 'tcp'] = cleaned.split('/');
	if (proto.toLowerCase() !== 'tcp') return null;
	const last = spec.split(':').pop() ?? '';
	// Ranges ("3000-3005") → first port.
	const n = Number(last.split('-')[0]);
	return Number.isInteger(n) && n > 0 && n < 65536 ? n : null;
}

/**
 * Container ports a compose file publishes or exposes, in order of appearance.
 * A line-based reader (not a YAML parser): good enough for hints.
 */
export function parseComposePorts(content: string): number[] {
	const ports: number[] = [];
	let inList: 'ports' | 'expose' | null = null;
	let listIndent = -1;
	for (const raw of content.split('\n')) {
		if (!raw.trim() || raw.trim().startsWith('#')) continue;
		const indent = raw.length - raw.trimStart().length;
		const line = raw.trim();
		const key = /^(ports|expose):\s*(.*)$/.exec(line);
		if (key) {
			inList = key[1] as 'ports' | 'expose';
			listIndent = indent;
			// Flow style: ports: ["8080:80"]
			const flow = /^\[(.*)\]$/.exec(key[2].trim());
			if (flow) {
				for (const item of flow[1].split(',')) {
					const p = containerPortOf(item);
					if (p) ports.push(p);
				}
				inList = null;
			}
			continue;
		}
		if (inList && indent > listIndent) {
			const target = /^(?:-\s*)?target:\s*(\d+)/.exec(line);
			if (target) {
				ports.push(Number(target[1]));
				continue;
			}
			const item = /^-\s*(.+)$/.exec(line);
			if (item && !item[1].includes(': ')) {
				const p = containerPortOf(item[1]);
				if (p) ports.push(p);
			}
			continue;
		}
		inList = null;
	}
	return [...new Set(ports)];
}

/** Mirrors tond-server's docker::default_memory_limit_for: 75% of RAM, at least 256 MiB. */
export function defaultMemoryCap(totalBytes: number): number {
	const MIN = 256 * 1024 * 1024;
	return Math.max(Math.floor(totalBytes / 4) * 3, MIN);
}

/** "1.5 GB", "512 MB". */
export function formatBytes(bytes: number): string {
	const mb = bytes / (1024 * 1024);
	if (mb >= 1024) {
		const gb = mb / 1024;
		return `${gb >= 10 ? Math.round(gb) : Math.round(gb * 10) / 10} GB`;
	}
	return `${Math.round(mb)} MB`;
}
