// Parse pasted `.env` blocks and describe what applying them would change.
//
// Supported syntax (the common dotenv subset):
//   KEY=value                 unquoted; trailing ` # comment` is stripped
//   export KEY=value          `export` prefix is ignored
//   KEY="a \"quoted\" value"  double quotes: \n \r \t \" \\ escapes, may span lines
//   KEY='literal $value'      single quotes: taken literally, may span lines
//   # comment / blank lines   ignored

const KEY_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/;

export function isValidEnvKey(key: string): boolean {
	return KEY_PATTERN.test(key);
}

export interface DotenvError {
	/** 1-based line number where the problem starts */
	line: number;
	message: string;
}

export interface DotenvResult {
	/** Parsed variables; a key repeated later in the block wins */
	vars: Record<string, string>;
	errors: DotenvError[];
}

function unescapeDouble(value: string): string {
	return value.replace(/\\([nrt"\\])/g, (_, ch: string) => {
		switch (ch) {
			case 'n':
				return '\n';
			case 'r':
				return '\r';
			case 't':
				return '\t';
			default:
				return ch;
		}
	});
}

/** Index of the closing quote in `text` starting at `from`, honouring backslash escapes for `"`. */
function findClosingQuote(text: string, quote: string, from: number): number {
	for (let i = from; i < text.length; i++) {
		if (quote === '"' && text[i] === '\\') {
			i++;
			continue;
		}
		if (text[i] === quote) return i;
	}
	return -1;
}

export function parseDotenv(text: string): DotenvResult {
	const vars: Record<string, string> = {};
	const errors: DotenvError[] = [];
	const lines = text.replace(/\r\n?/g, '\n').split('\n');

	for (let i = 0; i < lines.length; i++) {
		const lineNo = i + 1;
		const trimmed = lines[i].trim();
		if (!trimmed || trimmed.startsWith('#')) continue;

		const body = trimmed.replace(/^export\s+/, '');
		const eq = body.indexOf('=');
		if (eq === -1) {
			errors.push({ line: lineNo, message: `Expected KEY=VALUE, got "${truncate(trimmed)}"` });
			continue;
		}

		const key = body.slice(0, eq).trim();
		if (!isValidEnvKey(key)) {
			errors.push({
				line: lineNo,
				message: `"${truncate(key)}" is not a valid variable name`
			});
			continue;
		}

		const rest = body.slice(eq + 1).trimStart();
		const quote = rest[0];
		if (quote === '"' || quote === "'") {
			// Quoted values may continue over the following lines.
			let buffer = rest;
			let close = findClosingQuote(buffer, quote, 1);
			let j = i;
			while (close === -1 && j + 1 < lines.length) {
				j++;
				buffer += '\n' + lines[j];
				close = findClosingQuote(buffer, quote, 1);
			}
			if (close === -1) {
				errors.push({ line: lineNo, message: `Missing closing ${quote} for ${key}` });
				i = j;
				continue;
			}
			const trailing = buffer.slice(close + 1).trim();
			if (trailing && !trailing.startsWith('#')) {
				errors.push({
					line: lineNo,
					message: `Unexpected text after the closing quote of ${key}`
				});
				i = j;
				continue;
			}
			const raw = buffer.slice(1, close);
			vars[key] = quote === '"' ? unescapeDouble(raw) : raw;
			i = j;
			continue;
		}

		// Unquoted: an inline comment needs whitespace before the #.
		const comment = rest.search(/\s#/);
		vars[key] = (comment === -1 ? rest : rest.slice(0, comment)).trim();
	}

	return { vars, errors };
}

export interface EnvImportPlan {
	/** Keys that do not exist yet */
	added: string[];
	/** Existing keys whose value will be replaced */
	overwritten: string[];
}

/** Describe what setting `incoming` on top of `existingKeys` would change. */
export function planEnvImport(
	existingKeys: Iterable<string>,
	incoming: Record<string, string>
): EnvImportPlan {
	const existing = new Set(existingKeys);
	const added: string[] = [];
	const overwritten: string[] = [];
	for (const key of Object.keys(incoming).sort()) {
		(existing.has(key) ? overwritten : added).push(key);
	}
	return { added, overwritten };
}

function truncate(value: string, max = 40): string {
	return value.length > max ? `${value.slice(0, max)}…` : value;
}
