// A trimmed copy of docs/openapi.json for the mock API (regenerate by hand when the
// shape the API page reads changes; the real document is served by tond-server).
import type { OpenApiDocument } from '$lib/api';

export const MOCK_OPENAPI: OpenApiDocument = {
	openapi: '3.1.0',
	info: {
		title: 'Tond API',
		version: '1.0.0'
	},
	tags: [
		{
			description: 'Create, configure and control apps.',
			name: 'Apps'
		},
		{
			description: 'Deploy, follow and roll back.',
			name: 'Deployments'
		},
		{
			description: 'Environment variables (values are encrypted at rest).',
			name: 'Environment'
		},
		{
			description: 'Git forges, registries, webhooks and deploy keys.',
			name: 'Sources'
		},
		{
			description: 'Preview deployments of branches and pull requests.',
			name: 'Previews'
		},
		{
			description: 'Managed databases.',
			name: 'Databases'
		},
		{
			description: 'Scheduled container runs.',
			name: 'Cron'
		},
		{
			description: 'Groups of apps, databases and cron jobs.',
			name: 'Projects'
		},
		{
			description: 'Encrypted snapshots, restores and storage providers.',
			name: 'Snapshots'
		},
		{
			description: 'Metrics, traffic, events, access logs and uptime.',
			name: 'Observability'
		},
		{
			description: 'Alert rules, live alerts and history.',
			name: 'Alerts'
		},
		{
			description: 'Notification channels.',
			name: 'Notifications'
		},
		{
			description: 'The host, Docker disk, heartbeat and Tond itself.',
			name: 'System'
		},
		{
			description: 'Server-wide settings.',
			name: 'Settings'
		},
		{
			description: 'TLS certificates.',
			name: 'Certificates'
		},
		{
			description: 'Browser terminals into containers and the host.',
			name: 'Terminal'
		},
		{
			description: 'Dashboard sign-in, sessions and two-factor auth.',
			name: 'Auth'
		},
		{
			description: 'Dashboard users.',
			name: 'Users'
		},
		{
			description: 'Scoped API tokens.',
			name: 'Tokens'
		},
		{
			description: 'The hash-chained audit log and its shipping.',
			name: 'Audit log'
		},
		{
			description: 'Endpoints called by Git forges (signed, no token).',
			name: 'Webhooks'
		},
		{
			description: 'Probes, metrics and this document.',
			name: 'Meta'
		}
	],
	paths: {
		'/api/v1/alerts': {
			get: {
				operationId: 'list_alerts',
				summary: 'List alert rules',
				tags: ['Alerts'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['metrics:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			post: {
				operationId: 'create_alert',
				summary: 'Create an alert rule',
				tags: ['Alerts'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'201': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/alerts/active': {
			get: {
				operationId: 'list_active',
				summary: 'Alerts firing now',
				tags: ['Alerts'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['metrics:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/alerts/history': {
			get: {
				operationId: 'list_history',
				summary: 'Alert history',
				tags: ['Alerts'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['metrics:read'],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/alerts/kinds': {
			get: {
				operationId: 'list_kinds',
				summary: 'Alert kinds and their parameters',
				tags: ['Alerts'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['metrics:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/alerts/settings': {
			get: {
				operationId: 'alerts_get_settings',
				summary: 'Global alert settings (quiet hours)',
				tags: ['Alerts'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['metrics:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			put: {
				operationId: 'put_settings',
				summary: 'Update global alert settings',
				tags: ['Alerts'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/alerts/{id}': {
			delete: {
				operationId: 'delete_alert',
				summary: 'Delete an alert rule',
				tags: ['Alerts'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			},
			get: {
				operationId: 'get_alert',
				summary: 'Get an alert rule',
				tags: ['Alerts'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['metrics:read'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'429': {},
					'500': {}
				}
			},
			patch: {
				operationId: 'update_alert',
				summary: 'Update an alert rule',
				tags: ['Alerts'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/apps': {
			get: {
				operationId: 'list_apps',
				summary: 'List apps',
				tags: ['Apps'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:read'],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			post: {
				operationId: 'create_app',
				summary: 'Create an app',
				description:
					'Volumes and Docker Compose sources also need `settings:write`, because they reach the host.',
				tags: ['Apps'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'201': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/apps/{id}': {
			delete: {
				operationId: 'delete_app',
				summary: 'Delete an app, its containers, images and volumes',
				tags: ['Apps'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:delete'],
				parameters: [
					{
						description: 'App id or name.',
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			},
			get: {
				operationId: 'get_app',
				summary: 'Get an app (by id or name)',
				tags: ['Apps'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:read'],
				parameters: [
					{
						description: 'App id or name.',
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/apps/{id}/deploy': {
			post: {
				operationId: 'deploy_app',
				summary: 'Deploy an app from its source',
				description:
					'Queues a deployment and returns at once. Follow it with GET /api/v1/apps/{id}/deployments/{deployment_id} or its /stream WebSocket (`tond apps deploy --wait`).',
				tags: ['Deployments'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:deploy'],
				parameters: [
					{
						description: 'App id or name.',
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'202': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/audit-log': {
			get: {
				operationId: 'list_audit_log',
				summary: 'Audit log, newest first',
				tags: ['Audit log'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['events:read'],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/audit-log/shipping': {
			get: {
				operationId: 'get_shipping',
				summary: 'Audit log shipping settings and status',
				tags: ['Audit log'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			put: {
				operationId: 'update_shipping',
				summary: 'Configure audit log shipping',
				tags: ['Audit log'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/audit-log/verify': {
			get: {
				operationId: 'verify_audit_log',
				summary: "Verify the audit log's hash chain",
				tags: ['Audit log'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['events:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/auth/login': {
			post: {
				operationId: 'login',
				summary: 'Sign in with email and password',
				tags: ['Auth'],
				security: [],
				'x-tond-scopes': [],
				responses: {
					'200': {},
					'400': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/auth/whoami': {
			get: {
				operationId: 'whoami',
				summary: 'Who the credentials belong to (user, token or local socket)',
				tags: ['Auth'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': [],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/certs': {
			get: {
				operationId: 'list_certs',
				summary: 'TLS certificates and their status',
				tags: ['Certificates'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/cron': {
			get: {
				operationId: 'list_cron_jobs',
				summary: 'List cron jobs',
				tags: ['Cron'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['cron:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			post: {
				operationId: 'create_cron_job',
				summary: 'Create a cron job',
				tags: ['Cron'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['cron:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'201': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/cron/schedule-preview': {
			get: {
				operationId: 'preview_schedule',
				summary: 'Next run times of a schedule',
				tags: ['Cron'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['cron:read'],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/cron/{id}': {
			delete: {
				operationId: 'delete_cron_job',
				summary: 'Delete a cron job',
				tags: ['Cron'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['cron:write'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			},
			get: {
				operationId: 'get_cron_job',
				summary: 'Get a cron job',
				tags: ['Cron'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['cron:read'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'429': {},
					'500': {}
				}
			},
			patch: {
				operationId: 'update_cron_job',
				summary: 'Update a cron job',
				tags: ['Cron'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['cron:write'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/databases': {
			get: {
				operationId: 'list_databases',
				summary: 'List databases',
				tags: ['Databases'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['db:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			post: {
				operationId: 'create_database',
				summary: 'Create a database',
				description:
					'The credentials are in the response; reveal them later with GET /api/v1/databases/{id}/credentials (`db:credentials`).',
				tags: ['Databases'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['db:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'201': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/databases/{id}': {
			delete: {
				operationId: 'delete_database',
				summary: 'Delete a database and its data',
				tags: ['Databases'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['db:delete'],
				parameters: [
					{
						description: 'Database id or name.',
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			},
			get: {
				operationId: 'get_database',
				summary: 'Get a database',
				tags: ['Databases'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['db:read'],
				parameters: [
					{
						description: 'Database id or name.',
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'429': {},
					'500': {}
				}
			},
			patch: {
				operationId: 'update_database',
				summary: "Update a database's settings",
				tags: ['Databases'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['db:write'],
				parameters: [
					{
						description: 'Database id or name.',
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/integrations': {
			get: {
				operationId: 'list_integrations',
				summary: 'List notification channels',
				tags: ['Notifications'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			post: {
				operationId: 'create_integration',
				summary: 'Add a notification channel',
				tags: ['Notifications'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'201': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/integrations/{id}': {
			delete: {
				operationId: 'delete_integration',
				summary: 'Delete a notification channel',
				tags: ['Notifications'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			},
			get: {
				operationId: 'get_integration',
				summary: 'Get a notification channel',
				tags: ['Notifications'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'429': {},
					'500': {}
				}
			},
			patch: {
				operationId: 'update_integration',
				summary: 'Update a notification channel',
				tags: ['Notifications'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/openapi.json': {
			get: {
				operationId: 'spec',
				summary: 'This OpenAPI document',
				tags: ['Meta'],
				security: [],
				'x-tond-scopes': [],
				responses: {
					'200': {},
					'500': {}
				}
			}
		},
		'/api/v1/projects': {
			get: {
				operationId: 'list_projects',
				summary: 'List projects',
				tags: ['Projects'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			post: {
				operationId: 'create_project',
				summary: 'Create a project',
				tags: ['Projects'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'201': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/projects/{id}': {
			delete: {
				operationId: 'delete_project',
				summary: 'Delete a project (its apps are kept)',
				tags: ['Projects'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:delete'],
				parameters: [
					{
						description: 'Project id or name.',
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			},
			get: {
				operationId: 'get_project',
				summary: 'A project with its apps, databases and cron jobs',
				tags: ['Projects'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:read'],
				parameters: [
					{
						description: 'Project id or name.',
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'429': {},
					'500': {}
				}
			},
			put: {
				operationId: 'update_project',
				summary: 'Update a project',
				tags: ['Projects'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:write'],
				parameters: [
					{
						description: 'Project id or name.',
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/projects/{id}/deploy': {
			post: {
				operationId: 'deploy_project',
				summary: 'Deploy every app in a project',
				tags: ['Projects'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['apps:deploy'],
				parameters: [
					{
						description: 'Project id or name.',
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'202': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/settings': {
			get: {
				operationId: 'settings_get_settings',
				summary: 'Server settings',
				tags: ['Settings'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': [],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			put: {
				operationId: 'update_settings',
				summary: 'Update server settings',
				tags: ['Settings'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/settings/host-terminal': {
			get: {
				operationId: 'get_host_terminal',
				summary: 'Whether the host terminal is enabled',
				tags: ['Settings'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': [],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			put: {
				operationId: 'set_host_terminal',
				summary: 'Enable or disable the host terminal',
				description: 'Only over the root-only Unix socket (`tond settings host-terminal on|off`).',
				tags: ['Settings'],
				security: [],
				'x-tond-scopes': [],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'500': {}
				}
			}
		},
		'/api/v1/settings/retention': {
			get: {
				operationId: 'get_retention',
				summary: 'Data retention rules and table sizes',
				tags: ['Settings'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': [],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			put: {
				operationId: 'update_retention',
				summary: 'Update data retention',
				tags: ['Settings'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/snapshots': {
			get: {
				operationId: 'list_snapshots',
				summary: 'List snapshots',
				tags: ['Snapshots'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['snapshots:read'],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			post: {
				operationId: 'create_snapshot',
				summary: 'Take a snapshot now',
				tags: ['Snapshots'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['snapshots:create'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'201': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/snapshots/config': {
			get: {
				operationId: 'get_backup_config',
				summary: 'Snapshot schedule and retention',
				tags: ['Snapshots'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['snapshots:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			put: {
				operationId: 'update_backup_config',
				summary: 'Update the snapshot schedule and retention',
				tags: ['Snapshots'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['settings:write'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/snapshots/{id}': {
			delete: {
				operationId: 'delete_snapshot',
				summary: 'Delete a snapshot',
				tags: ['Snapshots'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['snapshots:restore'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/system/self': {
			get: {
				operationId: 'get_self',
				summary: "Tond's own resource use, loops, DB and HTTP latency",
				tags: ['System'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['metrics:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/terminal/tickets': {
			post: {
				operationId: 'create_ticket',
				summary: 'Get a one-time ticket to open a terminal',
				description:
					'A host terminal also needs `terminal:host`, the setting turned on and your password (step-up).',
				tags: ['Terminal'],
				security: [],
				'x-tond-scopes': [],
				responses: {
					'201': {},
					'400': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/tokens': {
			get: {
				operationId: 'list_tokens',
				summary: 'List API tokens',
				description: "Your own tokens; `all=true` lists everyone's and needs `tokens:admin`.",
				tags: ['Tokens'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': [],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			post: {
				operationId: 'create_token',
				summary: 'Create an API token',
				description:
					'The secret is in the response once and never again. Scopes are capped by your role; `tokens:admin` may create tokens for others.',
				tags: ['Tokens'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': [],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'201': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/tokens/catalog': {
			get: {
				operationId: 'token_catalog',
				summary: 'Scopes, groups, presets and roles for the token form',
				tags: ['Tokens'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': [],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/tokens/{id}': {
			delete: {
				operationId: 'revoke_token',
				summary: 'Revoke an API token',
				tags: ['Tokens'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': [],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/uptime': {
			get: {
				operationId: 'list_checks',
				summary: 'Uptime of every check',
				tags: ['Observability'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['metrics:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/users': {
			get: {
				operationId: 'list_users',
				summary: 'List users',
				tags: ['Users'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['users:admin'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			},
			post: {
				operationId: 'create_user',
				summary: 'Invite a user',
				tags: ['Users'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['users:admin'],
				parameters: [
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'201': {},
					'400': {},
					'401': {},
					'403': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/api/v1/users/{id}': {
			delete: {
				operationId: 'delete_user',
				summary: 'Delete a user',
				tags: ['Users'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['users:admin'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			},
			patch: {
				operationId: 'update_user',
				summary: "Change a user's role or disable them",
				tags: ['Users'],
				security: [
					{
						bearerAuth: []
					},
					{
						csrfHeader: [],
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['users:admin'],
				parameters: [
					{
						in: 'path',
						name: 'id',
						required: true,
						schema: {
							type: 'string'
						}
					},
					{
						$ref: '#/components/parameters/IdempotencyKey'
					}
				],
				responses: {
					'200': {},
					'400': {},
					'401': {},
					'403': {},
					'404': {},
					'409': {},
					'422': {},
					'429': {},
					'500': {}
				}
			}
		},
		'/health': {
			get: {
				operationId: 'health',
				summary: 'Liveness probe',
				tags: ['Meta'],
				security: [],
				'x-tond-scopes': [],
				responses: {
					'200': {},
					'500': {}
				}
			}
		},
		'/metrics': {
			get: {
				operationId: 'metrics',
				summary: 'Prometheus metrics',
				description: 'Needs a token with `metrics:read` over TCP; the root socket needs none.',
				tags: ['Meta'],
				security: [
					{
						bearerAuth: []
					},
					{
						sessionCookie: []
					}
				],
				'x-tond-scopes': ['metrics:read'],
				responses: {
					'200': {},
					'401': {},
					'403': {},
					'429': {},
					'500': {}
				}
			}
		}
	},
	components: {
		schemas: {
			ErrorCode: {
				description:
					'Stable error code. Each has a fixed HTTP status and `tond` CLI exit code (x-tond-codes, x-tond-exit-codes). New codes may be added within v1; handle unknown codes by HTTP status.',
				enum: [
					'INVALID_INPUT',
					'INVALID_JSON',
					'METHOD_NOT_ALLOWED',
					'PAYLOAD_TOO_LARGE',
					'UNAUTHENTICATED',
					'SESSION_EXPIRED',
					'INVALID_CREDENTIALS',
					'TOKEN_INVALID',
					'TOKEN_REVOKED',
					'TOKEN_EXPIRED',
					'ACCOUNT_DISABLED',
					'IP_NOT_ALLOWED',
					'CSRF_REJECTED',
					'FORBIDDEN',
					'SCOPE_DENIED',
					'RESOURCE_DENIED',
					'APPROVAL_REQUIRED',
					'NOT_FOUND',
					'APP_NOT_FOUND',
					'ROUTE_NOT_FOUND',
					'CONFLICT',
					'APP_ALREADY_EXISTS',
					'DEPLOY_IN_PROGRESS',
					'IDEMPOTENCY_KEY_REUSED',
					'IDEMPOTENCY_KEY_IN_USE',
					'BUILD_FAILED',
					'DEPLOY_FAILED',
					'RATE_LIMITED',
					'DOCKER_ERROR',
					'DATABASE_ERROR',
					'INTERNAL',
					'SERVICE_UNAVAILABLE'
				],
				type: 'string',
				'x-tond-codes': [
					{
						code: 'INVALID_INPUT',
						description: 'A value in the request is invalid; `field` names it when known.',
						exit_code: 2,
						status: 400
					},
					{
						code: 'INVALID_JSON',
						description: 'The request body is not valid JSON for this endpoint.',
						exit_code: 2,
						status: 400
					},
					{
						code: 'METHOD_NOT_ALLOWED',
						description: 'The path exists but not with this HTTP method.',
						exit_code: 2,
						status: 405
					},
					{
						code: 'PAYLOAD_TOO_LARGE',
						description: 'The request body exceeds a configured limit.',
						exit_code: 2,
						status: 413
					},
					{
						code: 'UNAUTHENTICATED',
						description: 'No credentials were sent.',
						exit_code: 4,
						status: 401
					},
					{
						code: 'SESSION_EXPIRED',
						description: 'The dashboard session ended.',
						exit_code: 4,
						status: 401
					},
					{
						code: 'INVALID_CREDENTIALS',
						description: 'A password, one-time code, setup token or link was wrong.',
						exit_code: 4,
						status: 401
					},
					{
						code: 'TOKEN_INVALID',
						description: 'The API token is unknown or malformed.',
						exit_code: 4,
						status: 401
					},
					{
						code: 'TOKEN_REVOKED',
						description: 'The API token was revoked.',
						exit_code: 4,
						status: 401
					},
					{
						code: 'TOKEN_EXPIRED',
						description: 'The API token expired.',
						exit_code: 4,
						status: 401
					},
					{
						code: 'ACCOUNT_DISABLED',
						description: "The account (or the token's owner) is disabled.",
						exit_code: 4,
						status: 401
					},
					{
						code: 'IP_NOT_ALLOWED',
						description: "The API token can't be used from this address.",
						exit_code: 4,
						status: 403
					},
					{
						code: 'CSRF_REJECTED',
						description: 'A browser request failed the same-origin checks.',
						exit_code: 4,
						status: 403
					},
					{
						code: 'FORBIDDEN',
						description: "The caller's role can't do this.",
						exit_code: 4,
						status: 403
					},
					{
						code: 'SCOPE_DENIED',
						description:
							'The role or token lacks the scope this operation needs (see x-tond-scopes).',
						exit_code: 4,
						status: 403
					},
					{
						code: 'RESOURCE_DENIED',
						description: 'The token is limited to other apps or projects.',
						exit_code: 4,
						status: 403
					},
					{
						code: 'APPROVAL_REQUIRED',
						description: 'The action needs a human approval before it runs.',
						exit_code: 7,
						status: 403
					},
					{
						code: 'NOT_FOUND',
						description: 'The resource does not exist.',
						exit_code: 3,
						status: 404
					},
					{
						code: 'APP_NOT_FOUND',
						description: 'No app has this id or name.',
						exit_code: 3,
						status: 404
					},
					{
						code: 'ROUTE_NOT_FOUND',
						description: 'No API endpoint matches this path.',
						exit_code: 3,
						status: 404
					},
					{
						code: 'CONFLICT',
						description: "The request conflicts with the resource's current state.",
						exit_code: 5,
						status: 409
					},
					{
						code: 'APP_ALREADY_EXISTS',
						description: 'An app with this name already exists.',
						exit_code: 5,
						status: 409
					},
					{
						code: 'DEPLOY_IN_PROGRESS',
						description: 'A deploy of this app is running; the action has to wait for it.',
						exit_code: 5,
						status: 409
					},
					{
						code: 'IDEMPOTENCY_KEY_REUSED',
						description: 'The Idempotency-Key was already used with a different request.',
						exit_code: 5,
						status: 422
					},
					{
						code: 'IDEMPOTENCY_KEY_IN_USE',
						description: 'A request with this Idempotency-Key is still being processed.',
						exit_code: 5,
						status: 409
					},
					{
						code: 'BUILD_FAILED',
						description: 'The image build failed.',
						exit_code: 6,
						status: 422
					},
					{
						code: 'DEPLOY_FAILED',
						description:
							'The deploy failed after the build (container start, health check or cutover).',
						exit_code: 6,
						status: 422
					},
					{
						code: 'RATE_LIMITED',
						description: 'Too many requests; see the Retry-After header.',
						exit_code: 8,
						status: 429
					},
					{
						code: 'DOCKER_ERROR',
						description: 'Docker failed to carry out the operation.',
						exit_code: 1,
						status: 500
					},
					{
						code: 'DATABASE_ERROR',
						description: "Tond's own database failed.",
						exit_code: 1,
						status: 500
					},
					{
						code: 'INTERNAL',
						description: 'An unexpected server error.',
						exit_code: 1,
						status: 500
					},
					{
						code: 'SERVICE_UNAVAILABLE',
						description: 'The server is starting, shutting down or temporarily unable to serve.',
						exit_code: 8,
						status: 503
					}
				],
				'x-tond-exit-codes': [
					{
						exit_code: 0,
						meaning: 'Success.',
						name: 'ok'
					},
					{
						exit_code: 1,
						meaning: 'Unexpected failure (server error, Docker error, anything not listed).',
						name: 'error'
					},
					{
						exit_code: 2,
						meaning:
							'Invalid arguments or input (bad flags, a value the server rejected, missing --yes without a TTY).',
						name: 'usage'
					},
					{
						exit_code: 3,
						meaning: 'The app, database, deployment or other resource does not exist.',
						name: 'not_found'
					},
					{
						exit_code: 4,
						meaning:
							'Not authenticated, the token is invalid/expired, or it lacks the required scope.',
						name: 'denied'
					},
					{
						exit_code: 5,
						meaning:
							'Conflicts with the current state (already exists, deploy in progress, idempotency key reused).',
						name: 'conflict'
					},
					{
						exit_code: 6,
						meaning: 'The build or deploy ran and failed (`deploy --wait`).',
						name: 'failed'
					},
					{
						exit_code: 7,
						meaning: 'The action waits for a human approval.',
						name: 'approval_required'
					},
					{
						exit_code: 8,
						meaning: 'The server could not be reached, is restarting, or rate-limited the request.',
						name: 'unavailable'
					}
				]
			}
		}
	}
};
