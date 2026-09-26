#!/usr/bin/env node
// Generate the reference pages that come from the code, before every build
// (the outputs are ignored by git, so they can't drift):
//
// From docs/openapi.json (OpenAPI 3.1, written by `just openapi` from the
// server's route table), under content/reference/api/:
//   index.md      overview: base URL, authentication, errors, one line per endpoint
//   <tag>.md      the endpoints of one tag: scopes, parameters, body, responses
//   schemas.md    every component schema as a field table
//   sidebar.json  the tag pages, for the site's navigation
//
// From templates/*.md, with their tables filled in:
//   content/reference/error-codes.md  <!-- table:error-codes --> from the spec's
//                                     ErrorCode schema (x-tond-codes)
//   content/reference/exit-codes.md   <!-- table:exit-codes --> and
//                                     <!-- table:local-codes --> from
//                                     src/data/exit-codes.json (`tond help
//                                     exit-codes --json`, refreshed by `just docs-gen`)
//
// The pages are plain Markdown, so they are searchable, work without
// JavaScript and end up in llms-full.txt like every other page. Without an
// openapi.json the API index says so and no tag pages exist.
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const SPEC = fileURLToPath(new URL('../openapi.json', import.meta.url));
const OUT = fileURLToPath(new URL('../content/reference/api/', import.meta.url));
const TEMPLATES = fileURLToPath(new URL('../templates/', import.meta.url));
const REFERENCE = fileURLToPath(new URL('../content/reference/', import.meta.url));
const EXIT_CODES = fileURLToPath(new URL('../src/data/exit-codes.json', import.meta.url));
// Served as /docs/openapi.json next to the rendered pages.
const PUBLIC_SPEC = fileURLToPath(new URL('../public/openapi.json', import.meta.url));

const METHODS = ['get', 'put', 'post', 'delete', 'patch', 'head', 'options'];

export function slugify(text) {
	return String(text)
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '');
}

/** Heading id the site generates (github-slugger) for "GET /api/apps/{id}". */
export function operationAnchor(method, path) {
	return `${method}-${path}`
		.toLowerCase()
		.replace(/[^\p{L}\p{N}\s_-]/gu, '')
		.replace(/\s/g, '-');
}

function cell(text) {
	return String(text ?? '')
		.replace(/\|/g, '\\|')
		.replace(/\r?\n+/g, ' ')
		.trim();
}

function refName(ref) {
	return ref.split('/').pop();
}

/** A short, readable type for a JSON schema: `string`, `App[]`, `"a" | "b"`, … */
export function typeOf(schema, link = (name) => `[${name}](/reference/api/schemas#${slugify(name)})`) {
	if (!schema || typeof schema !== 'object') return 'any';
	if (schema.$ref) return link(refName(schema.$ref));
	if (schema.enum) return schema.enum.map((v) => `\`${JSON.stringify(v)}\``).join(' \\| ');
	if (schema.const !== undefined) return `\`${JSON.stringify(schema.const)}\``;
	for (const key of ['oneOf', 'anyOf']) {
		if (Array.isArray(schema[key])) {
			return schema[key].map((s) => typeOf(s, link)).join(' \\| ');
		}
	}
	if (Array.isArray(schema.allOf)) return schema.allOf.map((s) => typeOf(s, link)).join(' & ');
	let type = schema.type;
	if (Array.isArray(type)) {
		const nonNull = type.filter((t) => t !== 'null');
		const base =
			nonNull.length === 1 ? typeOf({ ...schema, type: nonNull[0] }, link) : nonNull.join(' \\| ');
		return type.includes('null') ? `${base} \\| null` : base;
	}
	if (type === 'array') return `${typeOf(schema.items, link)}[]`;
	if (type === 'object' && schema.additionalProperties && !schema.properties) {
		return `map of ${typeOf(schema.additionalProperties === true ? {} : schema.additionalProperties, link)}`;
	}
	if (type === 'integer' || type === 'number' || type === 'string') {
		return schema.format ? `${type} (${schema.format})` : type;
	}
	return type ?? 'any';
}

function fieldTable(schema, spec) {
	const resolved = schema?.$ref ? spec.components?.schemas?.[refName(schema.$ref)] : schema;
	if (!resolved?.properties) return '';
	const required = new Set(resolved.required ?? []);
	const rows = Object.entries(resolved.properties).map(
		([name, prop]) =>
			`| \`${name}\` | ${typeOf(prop)} | ${required.has(name) ? 'yes' : ''} | ${cell(prop.description)} |`
	);
	return ['| Field | Type | Required | Description |', '| --- | --- | --- | --- |', ...rows, ''].join(
		'\n'
	);
}

/** How an operation is authorized, as a sentence. */
function accessOf(op) {
	if (op['x-tond-local-socket-only']) {
		return '**Access:** only on the server, over the root-only Unix socket (`tond` as root).';
	}
	const scopes = op['x-tond-scopes'];
	if (Array.isArray(op.security) && op.security.length === 0 && !(scopes ?? []).length) {
		return '**Access:** public (no credentials).';
	}
	if (Array.isArray(scopes) && scopes.length) {
		return `**Scope:** ${scopes.map((s) => `\`${s}\``).join(', ')}`;
	}
	if (Array.isArray(scopes)) return '**Access:** any signed-in user or token.';
	return null;
}

/** The JSON a success response carries: the envelope's `data`, or the body. */
function successBody(media) {
	const schema = media?.schema;
	if (schema?.properties?.data && schema?.properties?.success) return schema.properties.data;
	return schema;
}

function renderOperation({ method, path, op }, spec) {
	const out = [`### ${method.toUpperCase()} ${path}`, ''];
	if (op.summary) out.push(op.summary.endsWith('.') ? op.summary : `${op.summary}.`, '');
	if (op.description && op.description !== op.summary) out.push(op.description, '');
	const access = accessOf(op);
	if (access) out.push(access, '');
	if (op['x-tond-websocket']) {
		out.push(
			`**WebSocket.** ${typeof op['x-tond-websocket'] === 'string' ? op['x-tond-websocket'] : 'Upgrade the connection; frames are JSON.'}`,
			''
		);
	}
	if (op.deprecated) out.push('> [!WARNING]', '> Deprecated.', '');

	const params = [...(spec.paths[path].parameters ?? []), ...(op.parameters ?? [])].map((p) =>
		p.$ref ? spec.components?.parameters?.[refName(p.$ref)] ?? p : p
	);
	if (params.length) {
		out.push('| Parameter | In | Type | Required | Description |', '| --- | --- | --- | --- | --- |');
		for (const p of params) {
			out.push(
				`| \`${p.name}\` | ${p.in} | ${typeOf(p.schema)} | ${p.required ? 'yes' : ''} | ${cell(p.description)} |`
			);
		}
		out.push('');
	}

	const body = op.requestBody?.content;
	if (body) {
		for (const [type, media] of Object.entries(body)) {
			out.push(`**Request body** (\`${type}\`): ${typeOf(media.schema)}`, '');
			const table = fieldTable(media.schema, spec);
			if (table) out.push(table);
		}
	}

	const responses = Object.entries(op.responses ?? {});
	if (responses.length) {
		out.push('| Status | Description | `data` |', '| --- | --- | --- |');
		for (const [status, res] of responses) {
			const r = res.$ref ? spec.components?.responses?.[refName(res.$ref)] ?? res : res;
			const media = r.content ? Object.values(r.content)[0] : null;
			const ok = /^2/.test(status);
			const body = ok ? successBody(media) : null;
			const shown = ok ? (body ? typeOf(body) : '') : '[error](/reference/error-codes)';
			out.push(`| ${status} | ${cell(r.description)} | ${shown} |`);
		}
		out.push('');
	}
	return out.join('\n');
}

export function render(spec) {
	const info = spec.info ?? {};
	const operations = [];
	for (const [path, item] of Object.entries(spec.paths ?? {})) {
		for (const method of METHODS) {
			if (item[method]) operations.push({ method, path, op: item[method] });
		}
	}
	const tagInfo = new Map((spec.tags ?? []).map((t) => [t.name, t]));
	const byTag = new Map();
	for (const o of operations) {
		const tag = o.op.tags?.[0] ?? 'Other';
		if (!byTag.has(tag)) byTag.set(tag, []);
		byTag.get(tag).push(o);
	}
	const tags = [...byTag.keys()].sort((a, b) => {
		const order = [...tagInfo.keys()];
		const ia = order.indexOf(a);
		const ib = order.indexOf(b);
		return (ia === -1 ? 1e9 : ia) - (ib === -1 ? 1e9 : ib) || a.localeCompare(b);
	});

	const files = {};
	const sidebar = [];
	for (const tag of tags) {
		const slug = slugify(tag) || 'other';
		const ops = byTag.get(tag);
		const description = tagInfo.get(tag)?.description ?? `The ${tag} endpoints.`;
		sidebar.push({ text: tag, link: `/reference/api/${slug}`, description });
		files[`${slug}.md`] = [
			`# ${tag}`,
			'',
			description,
			'',
			...ops.map((o) => renderOperation(o, spec))
		].join('\n');
	}

	const schemas = Object.entries(spec.components?.schemas ?? {});
	if (schemas.length) {
		sidebar.push({
			text: 'Schemas',
			link: '/reference/api/schemas',
			description: 'The objects the API sends and receives.'
		});
		files['schemas.md'] = [
			'# Schemas',
			'',
			'The objects the endpoints send and receive.',
			'',
			...schemas.map(([name, schema]) =>
				[
					`## ${name}`,
					'',
					schema.description ?? '',
					'',
					fieldTable(schema, spec) || `Type: ${typeOf(schema)}`,
					''
				].join('\n')
			)
		].join('\n');
	}

	const index = [
		'# HTTP API',
		'',
		info.description ??
			'Everything the dashboard and the CLI do goes through this API, so anything you can click you can script.',
		'',
		`This reference is generated from the OpenAPI ${spec.openapi ?? '3.1'} document the server describes itself with (version ${info.version ?? 'unknown'}). Download it here ([openapi.json](/openapi.json)), or from any server at \`/api/v1/openapi.json\`.`,
		'',
		'## Calling the API',
		'',
		'- **Base URL:** `https://<your dashboard domain>/api/v1`.',
		'- **Authentication:** an API token as a bearer token: `Authorization: Bearer tond_pat_…`. Create one in the dashboard or with `tond token create`; each token carries [scopes](/concepts/access) and each endpoint below names the scope it needs.',
		'- **On the server itself** the `tond` CLI talks to the same API over the root-only Unix socket `/opt/tond/data/tond.sock`, which needs no token: `curl --unix-socket /opt/tond/data/tond.sock http://tond/api/v1/apps`.',
		'- **Responses** are JSON: `{"success": true, "data": …}`, or on failure `{"success": false, "data": null, "error": {"code", "message", "hint", "field"}}`. See [Error codes](/reference/error-codes).',
		'- **Retries:** send an `Idempotency-Key` header with a mutating request and a retry with the same key returns the first result instead of acting twice (the CLI does this for you; `--idempotency-key` sets the key).',
		'- **Try it:** every Tond dashboard has an interactive API page at `/developer` (More → API), which runs requests against your own server with your session.',
		'',
		'## Endpoints',
		''
	];
	for (const tag of tags) {
		const slug = slugify(tag) || 'other';
		index.push(`### ${tag}`, '', '| Method | Path | Summary |', '| --- | --- | --- |');
		for (const o of byTag.get(tag)) {
			index.push(
				`| ${o.method.toUpperCase()} | [\`${o.path}\`](/reference/api/${slug}#${operationAnchor(o.method, o.path)}) | ${cell(o.op.summary)} |`
			);
		}
		index.push('');
	}
	files['index.md'] = index.join('\n');
	return { files, sidebar };
}

function placeholder() {
	return {
		files: {
			'index.md': [
				'# HTTP API',
				'',
				'Everything the dashboard and the CLI do goes through this API, so anything you can click you can script.',
				'',
				'> [!NOTE]',
				'> This build of the docs has no `docs/openapi.json`, so the endpoint reference is missing. Run `just openapi` and rebuild the docs to generate it.',
				'',
				'## Calling the API',
				'',
				'- **Base URL:** `https://<your dashboard domain>/api/v1`.',
				'- **Authentication:** an API token as a bearer token: `Authorization: Bearer tond_pat_…`. Create one in the dashboard or with `tond token create`; each token carries [scopes](/concepts/access).',
				'- **On the server itself** the `tond` CLI talks to the same API over the root-only Unix socket `/opt/tond/data/tond.sock`, which needs no token.',
				'- **Responses** are JSON: `{"success": true, "data": …}`, or on failure `{"success": false, "data": null, "error": {"code", "message", "hint", "field"}}`. See [Error codes](/reference/error-codes).',
				''
			].join('\n')
		},
		sidebar: []
	};
}

/** The error-code table: code, HTTP status, CLI exit code, meaning. */
export function errorCodesTable(spec) {
	const codes = spec?.components?.schemas?.ErrorCode?.['x-tond-codes'] ?? [];
	if (!codes.length) return '_The error-code table comes from docs/openapi.json, which this build doesn\'t have._';
	return [
		'| Code | HTTP | CLI exit | Meaning |',
		'| --- | --- | --- | --- |',
		...codes.map((c) => `| \`${c.code}\` | ${c.status} | ${c.exit_code} | ${cell(c.description)} |`)
	].join('\n');
}

/** The CLI's exit codes with the error codes behind each. */
export function exitCodesTable(data) {
	const rows = (data?.exit_codes ?? []).map((e) => {
		const codes = [...(e.error_codes ?? []), ...(e.local_codes ?? [])].map((c) => `\`${c}\``);
		return `| \`${e.exit_code}\` | \`${e.name}\` | ${cell(e.meaning)} | ${codes.join(', ')} |`;
	});
	return ['| Exit | Name | Meaning | Error codes |', '| --- | --- | --- | --- |', ...rows].join('\n');
}

/** Codes the CLI reports itself (never sent by the server). */
export function localCodesTable(data) {
	const rows = (data?.local_codes ?? []).map(
		(c) => `| \`${c.code}\` | ${c.exit_code} | ${cell(c.description)} |`
	);
	return ['| Code | CLI exit | Meaning |', '| --- | --- | --- |', ...rows].join('\n');
}

/** Fill `<!-- table:name -->` markers in a template. */
export function fillTemplate(text, tables) {
	return text.replace(/<!--\s*table:([a-z-]+)\s*-->/g, (marker, name) => {
		if (!(name in tables)) throw new Error(`Unknown table marker ${marker}`);
		return tables[name];
	});
}

function writeCodePages(spec) {
	const exitData = existsSync(EXIT_CODES) ? JSON.parse(readFileSync(EXIT_CODES, 'utf8')) : null;
	const tables = {
		'error-codes': errorCodesTable(spec),
		'exit-codes': exitCodesTable(exitData),
		'local-codes': localCodesTable(exitData)
	};
	for (const page of ['error-codes', 'exit-codes']) {
		const template = readFileSync(`${TEMPLATES}${page}.md`, 'utf8');
		writeFileSync(
			`${REFERENCE}${page}.md`,
			`<!-- Generated from docs/templates/${page}.md by scripts/generate.mjs. Edit the template. -->\n\n${fillTemplate(template, tables)}`
		);
	}
}

function main() {
	const { files, sidebar } = existsSync(SPEC)
		? render(JSON.parse(readFileSync(SPEC, 'utf8')))
		: placeholder();
	rmSync(OUT, { recursive: true, force: true });
	mkdirSync(OUT, { recursive: true });
	for (const [name, content] of Object.entries(files)) writeFileSync(`${OUT}${name}`, content);
	writeFileSync(`${OUT}sidebar.json`, `${JSON.stringify(sidebar, null, '\t')}\n`);
	if (existsSync(SPEC)) copyFileSync(SPEC, PUBLIC_SPEC);
	else rmSync(PUBLIC_SPEC, { force: true });
	writeCodePages(existsSync(SPEC) ? JSON.parse(readFileSync(SPEC, 'utf8')) : null);
	console.log(
		existsSync(SPEC)
			? `API reference: ${Object.keys(files).length} page(s) from openapi.json`
			: 'API reference: no docs/openapi.json, wrote a placeholder page'
	);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
