// Docker Compose apps: presentation helpers for services (status, usage,
// ports, domains) and per-service deploy progress.

import type { ComposeService, ComposeServiceSpec, DeploymentService } from '$lib/api';
import type { StatusTone } from '$lib/status';

/** What a StatusBadge shows for a service. */
export interface ServiceBadge {
	status: string;
	label: string;
	tone?: StatusTone;
}

/**
 * A service's badge: Docker's state, refined by its healthcheck and exit
 * code (a one-shot service that exited 0 is "Done", not failed).
 */
export function serviceBadge(
	svc: Pick<ComposeService, 'status' | 'health' | 'exit_code'>
): ServiceBadge {
	switch (svc.status) {
		case 'running':
			if (svc.health === 'unhealthy') return { status: 'unhealthy', label: 'Unhealthy' };
			if (svc.health === 'starting') return { status: 'starting', label: 'Starting' };
			if (svc.health === 'healthy') return { status: 'healthy', label: 'Healthy' };
			return { status: 'running', label: 'Running' };
		case 'exited':
			if (svc.exit_code === 0) return { status: 'completed', label: 'Done', tone: 'neutral' };
			return {
				status: 'failed',
				label: svc.exit_code != null ? `Exited (${svc.exit_code})` : 'Exited'
			};
		case 'restarting':
			return { status: 'restarting', label: 'Restarting', tone: 'warning' };
		case 'paused':
			return { status: 'paused', label: 'Paused' };
		case 'created':
			return { status: 'pending', label: 'Created' };
		case 'dead':
			return { status: 'dead', label: 'Dead' };
		case 'missing':
			return { status: 'stopped', label: 'Not running' };
		case 'pending':
			return { status: 'pending', label: 'Not deployed', tone: 'neutral' };
		case 'parked':
			return { status: 'parked', label: 'Parked', tone: 'neutral' };
		default:
			return { status: svc.status, label: svc.status };
	}
}

/** "2 of 3 running", counting finished one-shot services as fine. */
export function servicesSummary(all: ComposeService[]): string {
	const parked = all.filter((s) => s.parked).length;
	const services = all.filter((s) => !s.parked);
	const tail = parked ? ` · ${parked} parked` : '';
	if (services.length === 0) return parked ? `No services running${tail}` : 'No services';
	const up = services.filter(
		(s) => s.status === 'running' || (s.status === 'exited' && s.exit_code === 0)
	).length;
	if (up === services.length) {
		return (
			(services.length === 1 ? '1 service running' : `All ${services.length} services running`) +
			tail
		);
	}
	return `${up} of ${services.length} running${tail}`;
}

/** Memory in MiB/GiB, e.g. "84 MiB"; "—" when unknown. */
export function formatMemory(bytes: number | null | undefined): string {
	if (bytes == null) return '—';
	const mib = bytes / 1_048_576;
	if (mib >= 1024) return `${(mib / 1024).toFixed(1)} GiB`;
	if (mib >= 10) return `${Math.round(mib)} MiB`;
	return `${mib.toFixed(1)} MiB`;
}

/** CPU from hundredths of a percent of one core, e.g. 1250 → "12.5%". */
export function formatCpu(hundredths: number | null | undefined): string {
	if (hundredths == null) return '—';
	const pct = hundredths / 100;
	return `${pct >= 10 ? Math.round(pct) : pct.toFixed(1)}%`;
}

/** The container ports a service could publish: declared ones plus the current choice. */
export function portChoices(svc: Pick<ComposeService, 'ports' | 'published_port'>): number[] {
	const ports = [...svc.ports];
	if (svc.published_port != null && !ports.includes(svc.published_port)) {
		ports.push(svc.published_port);
	}
	return ports.sort((a, b) => a - b);
}

/** Parse a port typed by the user; null when it isn't 1–65535. */
export function parsePort(input: string): number | null {
	const trimmed = input.trim();
	if (!/^\d+$/.test(trimmed)) return null;
	const n = Number(trimmed);
	return n >= 1 && n <= 65535 ? n : null;
}

/** Normalise a domain as typed: trim, lowercase, drop a scheme, path and trailing dot. */
export function normalizeDomain(input: string): string {
	let d = input.trim().toLowerCase();
	d = d.replace(/^[a-z]+:\/\//, '');
	d = d.split('/')[0] ?? '';
	d = d.replace(/:\d+$/, '');
	return d.replace(/\.$/, '');
}

/** A plausible hostname (the server has the final word). */
export function isPlausibleDomain(domain: string): boolean {
	return /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9-]{2,63}$/.test(domain);
}

/** The web service the wizard suggests: the service declaring `port`, else the first with a port. */
export function suggestWebService(
	specs: ComposeServiceSpec[],
	port: number
): ComposeServiceSpec | null {
	return specs.find((s) => s.ports.includes(port)) ?? specs.find((s) => s.ports.length > 0) ?? null;
}

/** Label and tone of a service's step in a deployment. */
export function deploymentServiceBadge(
	s: Pick<DeploymentService, 'status' | 'source'>
): ServiceBadge {
	switch (s.status) {
		case 'building':
			return { status: 'building', label: 'Building' };
		case 'pulling':
			return { status: 'building', label: 'Pulling' };
		case 'ready':
			return {
				status: 'ready',
				label: s.source === 'reuse' ? 'Image kept' : s.source === 'build' ? 'Built' : 'Pulled',
				tone: 'neutral'
			};
		case 'starting':
			return { status: 'starting', label: 'Starting' };
		case 'healthy':
			return { status: 'healthy', label: 'Ready' };
		case 'running':
			return { status: 'running', label: 'Running' };
		case 'failed':
			return { status: 'failed', label: 'Failed' };
		default:
			return { status: 'pending', label: 'Waiting', tone: 'neutral' };
	}
}

/** Short form of an image digest: "sha256:4f1c2a…" → "4f1c2a3b9e01". */
export function shortDigest(digest: string | null | undefined): string | null {
	if (!digest) return null;
	const at = digest.lastIndexOf('sha256:');
	if (at < 0) return digest.slice(0, 12);
	return digest.slice(at + 7, at + 19);
}
