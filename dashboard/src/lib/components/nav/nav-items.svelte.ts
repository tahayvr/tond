export interface NavItem {
	href: string;
	label: string;
	icon?: string;
}

export const navItems: NavItem[] = [
	{ href: '/projects', label: 'Projects', icon: 'app' },
	{ href: '/apps', label: 'Apps', icon: 'rocket' },
	{ href: '/databases', label: 'Databases', icon: 'database' },
	{ href: '/cron', label: 'Cron_Jobs', icon: 'clock' },
	{ href: '/snapshots', label: 'Snapshots', icon: 'clock' },
	{ href: '/alerts', label: 'Alerts', icon: 'bell' },
	{ href: '/integrations', label: 'Integrations', icon: 'puzzle' },
	{ href: '/activity', label: 'Activity', icon: 'activity' },
	// { href: '/iam', label: 'IAM', icon: 'fingerprint-pattern' },
	{ href: '/system', label: 'System', icon: 'app' },
	{ href: '/settings', label: 'Settings', icon: 'settings' }
];

export default navItems;
