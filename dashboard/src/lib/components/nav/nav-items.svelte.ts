import { can } from '$lib/auth.svelte';

export interface NavItem {
	href: string;
	label: string;
	icon?: string;
	/** Only shown to users holding this scope */
	scope?: string;
}

export const navItems: NavItem[] = [
	{ href: '/projects', label: 'Projects', icon: 'app' },
	{ href: '/apps', label: 'Apps', icon: 'rocket' },
	{ href: '/databases', label: 'Databases', icon: 'database' },
	{ href: '/cron', label: 'Cron jobs', icon: 'clock' },
	{ href: '/snapshots', label: 'Snapshots', icon: 'clock' },
	{ href: '/alerts', label: 'Alerts', icon: 'bell' },
	{ href: '/certificates', label: 'Certificates', icon: 'lock' },
	{ href: '/integrations', label: 'Integrations', icon: 'puzzle', scope: 'settings:write' },
	{ href: '/activity', label: 'Activity', icon: 'activity' },
	// { href: '/iam', label: 'IAM', icon: 'fingerprint-pattern' },
	{ href: '/system', label: 'System', icon: 'app' },
	{ href: '/users', label: 'Users', icon: 'team', scope: 'users:admin' },
	{ href: '/settings', label: 'Settings', icon: 'settings' }
];

/** The nav items the signed-in user may use. */
export function visibleNavItems(): NavItem[] {
	return navItems.filter((item) => !item.scope || can(item.scope));
}

/** True when `pathname` is `href` or a page below it. */
export function isActive(href: string, pathname: string): boolean {
	return pathname === href || pathname.startsWith(href + '/');
}

export default navItems;
