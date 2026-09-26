// api-reference.ts — turn the server's OpenAPI document into what the API
// page shows: operations grouped by tag, the error-code catalog and the CLI
// exit codes. Pure functions (tested in api-reference.test.ts).

import type { OpenApiDocument, OpenApiOperation } from './api';

export type HttpMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';

const METHODS: HttpMethod[] = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'];

export interface ApiOperation {
	id: string;
	method: HttpMethod;
	path: string;
	summary: string;
	description: string | null;
	tag: string;
	/** Scopes the caller needs (empty for any signed-in caller). */
	scopes: string[];
	/** No credentials needed (probes, sign-in, forge webhooks). */
	public: boolean;
	websocket: boolean;
	/** Takes an Idempotency-Key. */
	idempotent: boolean;
	/** Success status (e.g. 200, 201, 202, 101). */
	status: string;
	/** Error statuses the operation documents. */
	errors: string[];
}

export interface ApiTagGroup {
	name: string;
	description: string | null;
	operations: ApiOperation[];
}

export interface ErrorCodeRow {
	code: string;
	status: number;
	exitCode: number;
	description: string;
}

export interface ExitCodeRow {
	exitCode: number;
	name: string;
	meaning: string;
}

export interface ApiReference {
	title: string;
	version: string;
	groups: ApiTagGroup[];
	errorCodes: ErrorCodeRow[];
	exitCodes: ExitCodeRow[];
	operationCount: number;
}

function toOperation(path: string, method: HttpMethod, op: OpenApiOperation): ApiOperation {
	const responses = Object.keys(op.responses ?? {});
	const success = responses.find((s) => s.startsWith('2') || s === '101') ?? '200';
	const params = op.parameters ?? [];
	return {
		id: op.operationId ?? `${method} ${path}`,
		method,
		path,
		summary: op.summary ?? '',
		description: op.description ?? null,
		tag: op.tags?.[0] ?? 'Other',
		scopes: op['x-tond-scopes'] ?? [],
		public: Array.isArray(op.security) && op.security.length === 0,
		websocket: op['x-tond-websocket'] === true,
		idempotent: params.some((p) => p.$ref === '#/components/parameters/IdempotencyKey'),
		status: success,
		errors: responses.filter((s) => s.startsWith('4') || s.startsWith('5'))
	};
}

/** Operations grouped by the document's tag order, paths sorted within. */
export function buildReference(doc: OpenApiDocument): ApiReference {
	const ops: ApiOperation[] = [];
	for (const [path, item] of Object.entries(doc.paths ?? {})) {
		for (const method of METHODS) {
			const op = item[method.toLowerCase() as Lowercase<HttpMethod>];
			if (op) ops.push(toOperation(path, method, op));
		}
	}
	const order = (doc.tags ?? []).map((t) => t.name);
	const descriptions = new Map((doc.tags ?? []).map((t) => [t.name, t.description ?? null]));
	const byTag = new Map<string, ApiOperation[]>();
	for (const op of ops) {
		const list = byTag.get(op.tag) ?? [];
		list.push(op);
		byTag.set(op.tag, list);
	}
	const rank = (tag: string) => {
		const i = order.indexOf(tag);
		return i === -1 ? order.length : i;
	};
	const groups = [...byTag.entries()]
		.sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b))
		.map(([name, operations]) => ({
			name,
			description: descriptions.get(name) ?? null,
			operations: operations.sort(
				(a, b) =>
					a.path.localeCompare(b.path) || METHODS.indexOf(a.method) - METHODS.indexOf(b.method)
			)
		}));

	const codeSchema = doc.components?.schemas?.ErrorCode;
	const errorCodes: ErrorCodeRow[] = (codeSchema?.['x-tond-codes'] ?? []).map((c) => ({
		code: c.code,
		status: c.status,
		exitCode: c.exit_code,
		description: c.description
	}));
	const exitCodes: ExitCodeRow[] = (codeSchema?.['x-tond-exit-codes'] ?? []).map((e) => ({
		exitCode: e.exit_code,
		name: e.name,
		meaning: e.meaning
	}));

	return {
		title: doc.info?.title ?? 'API',
		version: doc.info?.version ?? '',
		groups,
		errorCodes,
		exitCodes,
		operationCount: ops.length
	};
}

/**
 * Operations matching `query` (path, summary, scope, method or operation id;
 * every word must match) and, when given, the tag. Empty groups are dropped.
 */
export function filterReference(
	groups: ApiTagGroup[],
	query: string,
	tag: string | null = null
): ApiTagGroup[] {
	const words = query.toLowerCase().split(/\s+/).filter(Boolean);
	return groups
		.filter((g) => !tag || g.name === tag)
		.map((g) => ({
			...g,
			operations: g.operations.filter((op) => {
				if (words.length === 0) return true;
				const haystack = [op.method, op.path, op.summary, op.id, ...op.scopes]
					.join(' ')
					.toLowerCase();
				return words.every((w) => haystack.includes(w));
			})
		}))
		.filter((g) => g.operations.length > 0);
}

/** A curl example for an operation against `origin`. */
export function curlExample(op: ApiOperation, origin: string): string {
	const url = `${origin}${op.path}`;
	const parts = ['curl'];
	if (op.method !== 'GET') parts.push(`-X ${op.method}`);
	if (!op.public) parts.push('-H "Authorization: Bearer $TOND_TOKEN"');
	if (op.idempotent) parts.push('-H "Idempotency-Key: $(uuidgen)"');
	if (op.method === 'POST' || op.method === 'PUT' || op.method === 'PATCH') {
		parts.push("-H 'Content-Type: application/json' -d '{…}'");
	}
	parts.push(`"${url}"`);
	return parts.join(' ');
}

/** A run of API description text: plain, or `inline code`. */
export interface TextSegment {
	text: string;
	code: boolean;
}

/**
 * Split description text on backtick pairs (`like this`) into plain and code
 * segments. Render the segments as text nodes (never as HTML): a description
 * can't inject markup. An unpaired backtick stays as it is.
 */
export function inlineCodeSegments(text: string): TextSegment[] {
	const segments: TextSegment[] = [];
	let rest = text;
	for (;;) {
		const open = rest.indexOf('`');
		const close = open === -1 ? -1 : rest.indexOf('`', open + 1);
		if (close === -1) break;
		if (open > 0) segments.push({ text: rest.slice(0, open), code: false });
		if (close > open + 1) segments.push({ text: rest.slice(open + 1, close), code: true });
		rest = rest.slice(close + 1);
	}
	if (rest) segments.push({ text: rest, code: false });
	return segments;
}
