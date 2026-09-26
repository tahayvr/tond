import { describe, expect, it } from 'vitest';

import type { OpenApiDocument } from './api';
import { buildReference, curlExample, filterReference, inlineCodeSegments } from './api-reference';
import { MOCK_OPENAPI } from '../mocks/openapi';

const doc: OpenApiDocument = {
	openapi: '3.1.0',
	info: { title: 'Tond API', version: '1.0.0' },
	tags: [
		{ name: 'Apps', description: 'Apps.' },
		{ name: 'Deployments', description: 'Deploys.' }
	],
	paths: {
		'/api/v1/apps': {
			get: {
				operationId: 'list_apps',
				summary: 'List apps',
				tags: ['Apps'],
				security: [{ bearerAuth: [] }],
				'x-tond-scopes': ['apps:read'],
				responses: { '200': {}, '401': {}, '500': {} }
			},
			post: {
				operationId: 'create_app',
				summary: 'Create an app',
				tags: ['Apps'],
				security: [{ bearerAuth: [] }],
				'x-tond-scopes': ['apps:write'],
				parameters: [{ $ref: '#/components/parameters/IdempotencyKey' }],
				responses: { '201': {}, '400': {}, '500': {} }
			}
		},
		'/api/v1/apps/{id}/deploy': {
			post: {
				operationId: 'deploy_app',
				summary: 'Deploy an app from its source',
				tags: ['Deployments'],
				'x-tond-scopes': ['apps:deploy'],
				parameters: [{ $ref: '#/components/parameters/IdempotencyKey' }],
				responses: { '202': {}, '404': {} }
			}
		},
		'/health': {
			get: {
				operationId: 'health',
				summary: 'Liveness probe',
				security: [],
				responses: { '200': {} }
			}
		},
		'/api/v1/apps/{id}/logs/stream': {
			get: {
				operationId: 'stream_logs',
				summary: 'Live app logs',
				tags: ['Apps'],
				'x-tond-websocket': true,
				'x-tond-scopes': ['logs:read'],
				responses: { '101': {} }
			}
		}
	},
	components: {
		schemas: {
			ErrorCode: {
				'x-tond-codes': [
					{ code: 'APP_NOT_FOUND', status: 404, exit_code: 3, description: 'No app.' }
				],
				'x-tond-exit-codes': [{ exit_code: 3, name: 'not_found', meaning: 'Missing.' }]
			}
		}
	}
};

describe('buildReference', () => {
	it('groups operations by tag in the document order', () => {
		const ref = buildReference(doc);
		expect(ref.operationCount).toBe(5);
		expect(ref.groups.map((g) => g.name)).toEqual(['Apps', 'Deployments', 'Other']);
		const apps = ref.groups[0].operations;
		expect(apps.map((o) => `${o.method} ${o.path}`)).toEqual([
			'GET /api/v1/apps',
			'POST /api/v1/apps',
			'GET /api/v1/apps/{id}/logs/stream'
		]);
	});

	it('reads scopes, public access, websockets, idempotency and statuses', () => {
		const ref = buildReference(doc);
		const all = ref.groups.flatMap((g) => g.operations);
		const create = all.find((o) => o.id === 'create_app');
		expect(create).toMatchObject({
			scopes: ['apps:write'],
			public: false,
			idempotent: true,
			status: '201',
			errors: ['400', '500']
		});
		expect(all.find((o) => o.id === 'health')?.public).toBe(true);
		expect(all.find((o) => o.id === 'stream_logs')).toMatchObject({
			websocket: true,
			status: '101'
		});
		expect(ref.errorCodes).toEqual([
			{ code: 'APP_NOT_FOUND', status: 404, exitCode: 3, description: 'No app.' }
		]);
		expect(ref.exitCodes).toEqual([{ exitCode: 3, name: 'not_found', meaning: 'Missing.' }]);
	});

	it('handles the mock copy of the real document', () => {
		const ref = buildReference(MOCK_OPENAPI);
		expect(ref.operationCount).toBeGreaterThan(20);
		expect(ref.errorCodes.some((c) => c.code === 'DEPLOY_IN_PROGRESS')).toBe(true);
	});
});

describe('filterReference', () => {
	const ref = buildReference(doc);

	it('matches every word against path, summary, scope and method', () => {
		const hits = filterReference(ref.groups, 'deploy');
		expect(hits.flatMap((g) => g.operations.map((o) => o.id))).toEqual(['deploy_app']);
		expect(
			filterReference(ref.groups, 'post apps:write').flatMap((g) => g.operations.map((o) => o.id))
		).toEqual(['create_app']);
		expect(filterReference(ref.groups, 'nothing-like-this')).toEqual([]);
	});

	it('filters by tag and drops empty groups', () => {
		const hits = filterReference(ref.groups, '', 'Deployments');
		expect(hits.map((g) => g.name)).toEqual(['Deployments']);
	});
});

describe('curlExample', () => {
	it('adds the token, the idempotency key and a body where they apply', () => {
		const all = buildReference(doc).groups.flatMap((g) => g.operations);
		const create = all.find((o) => o.id === 'create_app');
		const health = all.find((o) => o.id === 'health');
		expect(create && curlExample(create, 'https://tond.example.com')).toBe(
			`curl -X POST -H "Authorization: Bearer $TOND_TOKEN" -H "Idempotency-Key: $(uuidgen)" -H 'Content-Type: application/json' -d '{…}' "https://tond.example.com/api/v1/apps"`
		);
		expect(health && curlExample(health, 'https://t.example')).toBe(
			'curl "https://t.example/health"'
		);
	});
});

describe('inlineCodeSegments', () => {
	it('splits backtick pairs into code segments', () => {
		expect(inlineCodeSegments('Keep `retention` (1–365) backups')).toEqual([
			{ text: 'Keep ', code: false },
			{ text: 'retention', code: true },
			{ text: ' (1–365) backups', code: false }
		]);
		expect(inlineCodeSegments('`a` and `b`')).toEqual([
			{ text: 'a', code: true },
			{ text: ' and ', code: false },
			{ text: 'b', code: true }
		]);
	});

	it('leaves plain text and unpaired backticks alone', () => {
		expect(inlineCodeSegments('plain')).toEqual([{ text: 'plain', code: false }]);
		expect(inlineCodeSegments('one ` only')).toEqual([{ text: 'one ` only', code: false }]);
		expect(inlineCodeSegments('')).toEqual([]);
	});
});
