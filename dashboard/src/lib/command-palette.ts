// Entries for the Cmd+K command palette, built from the resources the user can see.

import type { App, CronJob, ManagedDatabase, Project } from './api';
import { navItems } from './components/nav/nav-items.svelte.ts';
import { appTabHref } from './tabs';

export type PaletteGroup = 'Pages' | 'Apps' | 'Databases' | 'Cron jobs' | 'Projects' | 'Actions';

export type PaletteAction = { kind: 'deploy'; appId: string; appName: string } | { kind: 'theme' } | { kind: 'terminal' } | { kind: 'shortcuts' };

export interface PaletteEntry {
	id: string;
	group: PaletteGroup;
	label: string;
	/** Extra words the filter matches (status, type, id…). */
	keywords: string[];
	/** Secondary text shown on the right. */
	hint?: string;
	href?: string;
	action?: PaletteAction;
}

export interface PaletteData {
	apps: App[];
	databases: ManagedDatabase[];
	cronJobs: CronJob[];
	projects: Project[];
}

export function buildPaletteEntries(
	data: PaletteData,
	{ canDeploy = true }: { canDeploy?: boolean } = {}
): PaletteEntry[] {
	const entries: PaletteEntry[] = [
		{ id: 'page:/', group: 'Pages', label: 'Home', keywords: ['dashboard', 'overview'], href: '/' },
		...navItems.map((item) => ({
			id: `page:${item.href}`,
			group: 'Pages' as const,
			label: item.label,
			keywords: [item.href.slice(1)],
			href: item.href
		})),
		{ id: 'page:/profile', group: 'Pages', label: 'Profile', keywords: ['2fa', 'password', 'account'], href: '/profile' }
	];

	entries.push(
		{ id: 'new:app', group: 'Actions', label: 'New app', keywords: ['create', 'deploy'], href: '/apps/new' },
		{ id: 'new:db', group: 'Actions', label: 'New database', keywords: ['create', 'postgres', 'mysql', 'redis'], href: '/databases/new' },
		{ id: 'new:cron', group: 'Actions', label: 'New cron job', keywords: ['create', 'schedule'], href: '/cron/new' },
		{ id: 'new:alert', group: 'Actions', label: 'New alert rule', keywords: ['create', 'notify'], href: '/alerts/new' },
		{ id: 'act:theme', group: 'Actions', label: 'Toggle light/dark theme', keywords: ['dark', 'light', 'mode'], action: { kind: 'theme' } },
		{ id: 'act:terminal', group: 'Actions', label: 'Toggle terminal', keywords: ['shell', 'console'], action: { kind: 'terminal' } },
		{ id: 'act:shortcuts', group: 'Actions', label: 'Keyboard shortcuts', keywords: ['help', 'keys', 'hotkeys'], action: { kind: 'shortcuts' } }
	);

	for (const app of data.apps) {
		entries.push({
			id: `app:${app.id}`,
			group: 'Apps',
			label: app.name,
			keywords: [app.status, app.domain ?? '', 'app'],
			hint: app.status,
			href: `/apps/${app.id}`
		});
		entries.push({
			id: `logs:${app.id}`,
			group: 'Actions',
			label: `Open logs: ${app.name}`,
			keywords: ['logs', app.name],
			href: appTabHref(app.id, 'logs')
		});
		if (canDeploy) {
			entries.push({
				id: `deploy:${app.id}`,
				group: 'Actions',
				label: `Deploy ${app.name}`,
				keywords: ['deploy', 'redeploy', 'release', app.name],
				action: { kind: 'deploy', appId: app.id, appName: app.name }
			});
		}
	}
	for (const db of data.databases) {
		entries.push({
			id: `db:${db.id}`,
			group: 'Databases',
			label: db.name,
			keywords: [db.db_type, db.status, 'database'],
			hint: db.db_type,
			href: `/databases/${db.id}`
		});
	}
	for (const job of data.cronJobs) {
		entries.push({
			id: `cron:${job.id}`,
			group: 'Cron jobs',
			label: job.name,
			keywords: [job.schedule, 'cron', 'job'],
			hint: job.schedule,
			href: `/cron/${job.id}`
		});
	}
	for (const project of data.projects) {
		entries.push({
			id: `project:${project.id}`,
			group: 'Projects',
			label: project.name,
			keywords: ['project'],
			href: `/projects/${project.id}`
		});
	}
	return entries;
}

export const GROUP_ORDER: PaletteGroup[] = ['Actions', 'Apps', 'Databases', 'Cron jobs', 'Projects', 'Pages'];
