// The docs' table of contents. The sidebar, the prev/next links and the
// generated llms.txt / llms-full.txt all read it, so a page listed here is a
// page every reader (human or agent) can find, and a page missing from it
// fails the build (src/lib/pages.ts).
//
// Links are page ids: the path of the Markdown file under content/ without
// the extension ('/' is content/index.md). The HTTP API pages are generated
// from docs/openapi.json by scripts/gen-api.mjs and appended under "HTTP API".

/** @type {{ text: string, items: { text: string, link: string, description: string }[] }[]} */
export const sections = [
	{
		text: 'Getting started',
		items: [
			{
				text: 'Introduction',
				link: '/',
				description: 'What Tond is and the principles behind it.'
			},
			{
				text: 'Quickstart',
				link: '/quickstart',
				description: 'Install Tond on a VPS, deploy a first app and give it a domain.'
			},
			{
				text: 'Supported systems',
				link: '/support-matrix',
				description: 'Linux distributions, CPU architectures and server sizes Tond supports.'
			}
		]
	},
	{
		text: 'Concepts',
		items: [
			{
				text: 'Apps',
				link: '/concepts/apps',
				description: 'App kinds, sources, readiness and lifecycle.'
			},
			{
				text: 'Projects',
				link: '/concepts/projects',
				description: 'Grouping apps and databases.'
			},
			{
				text: 'Databases',
				link: '/concepts/databases',
				description: 'Managed PostgreSQL, MySQL, MariaDB, Redis, KeyDB and MongoDB next to your apps.'
			},
			{
				text: 'Deploys and rollback',
				link: '/concepts/deploys',
				description: 'How a deploy runs, the build queue, and rolling back.'
			},
			{
				text: 'Previews',
				link: '/concepts/previews',
				description: 'Pull-request preview deployments and the preview domain.'
			},
			{
				text: 'Domains and certificates',
				link: '/concepts/domains',
				description: 'Dashboard domain, apps domain, custom domains and TLS certificates.'
			},
			{
				text: 'Volumes',
				link: '/concepts/volumes',
				description: 'Persistent storage for apps.'
			},
			{
				text: 'Networks and database links',
				link: '/concepts/networks',
				description: 'Per-app Docker networks and how apps reach databases.'
			},
			{
				text: 'Environment and secrets',
				link: '/concepts/secrets',
				description: 'Environment variables, encrypted secrets and TOND_SECRET_KEY.'
			},
			{
				text: 'Snapshots and backups',
				link: '/concepts/backups',
				description: 'Local snapshots, remote S3 copies and restores.'
			},
			{
				text: 'Metrics, uptime and alerts',
				link: '/concepts/observability',
				description: 'What Tond measures, uptime checks, heartbeats and alert rules.'
			},
			{
				text: 'Users, roles and tokens',
				link: '/concepts/access',
				description: 'Accounts, roles, scopes and API tokens.'
			},
			{
				text: 'Audit log',
				link: '/concepts/audit-log',
				description: 'The hash-chained record of every change.'
			}
		]
	},
	{
		text: 'Guides',
		items: [
			{
				text: 'Deploy from Git',
				link: '/guides/deploy-from-git',
				description: 'Connect GitHub, GitLab or Gitea and deploy on push.'
			},
			{
				text: 'Build sources',
				link: '/guides/sources',
				description: 'Deploy a Dockerfile, a compose file, an image or a tarball.'
			},
			{
				text: 'Templates',
				link: '/guides/templates',
				description: 'Start from a ready-made app template.'
			},
			{
				text: 'Migrate a database',
				link: '/guides/migrate-database',
				description: 'Move an existing database into Tond.'
			},
			{
				text: 'Alerts and heartbeat',
				link: '/guides/alerts',
				description: 'Get notified when something breaks, including the whole server.'
			},
			{
				text: 'CI with a deployer token',
				link: '/guides/ci',
				description: 'Deploy from CI with a scoped API token.'
			},
			{
				text: 'Backups to S3',
				link: '/guides/backups-s3',
				description: 'Ship encrypted snapshots to S3-compatible storage.'
			},
			{
				text: 'Upgrading Tond',
				link: '/guides/upgrading',
				description: 'Update from the dashboard or the CLI, and roll back.'
			},
			{
				text: 'Host terminal',
				link: '/guides/host-terminal',
				description: 'The browser shell on the server and its limits.'
			},
			{
				text: 'Security hardening checklist',
				link: '/guides/hardening',
				description: 'Steps to lock down a Tond server.'
			}
		]
	},
	{
		text: 'Reference',
		items: [
			{
				text: 'CLI',
				link: '/reference/cli',
				description: 'Every tond command and flag (generated from the CLI).'
			},
			{
				text: 'HTTP API',
				link: '/reference/api/',
				description: 'Every API endpoint (generated from the OpenAPI document).'
			},
			{
				text: 'Configuration',
				link: '/reference/configuration',
				description: 'Environment variables, settings and installer flags.'
			},
			{
				text: 'Exit codes',
				link: '/reference/exit-codes',
				description: 'Exit codes of the tond CLI and the install and update scripts.'
			},
			{
				text: 'Error codes',
				link: '/reference/error-codes',
				description: 'Error codes returned by the HTTP API.'
			}
		]
	},
	{
		text: 'Operations',
		items: [
			{
				text: 'Resource footprint',
				link: '/operations/footprint',
				description: 'Memory, CPU and disk Tond itself uses.'
			},
			{
				text: 'Retention',
				link: '/operations/retention',
				description: 'How long logs, metrics and history are kept.'
			},
			{
				text: 'Observability',
				link: '/operations/observability',
				description: 'Logs, Prometheus metrics and self-monitoring.'
			},
			{
				text: 'Troubleshooting',
				link: '/operations/troubleshooting',
				description: 'Certificates, DNS, ports and firewalls.'
			}
		]
	},
	{
		text: 'Security',
		items: [
			{
				text: 'Threat model',
				link: '/security/threat-model',
				description: 'What Tond defends against and what it does not.'
			},
			{
				text: 'Isolation',
				link: '/security/isolation',
				description: 'What is isolated from what.'
			},
			{
				text: 'Reporting vulnerabilities',
				link: '/security/reporting',
				description: 'How to report a security issue.'
			}
		]
	}
];
