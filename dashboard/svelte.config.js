import adapter from '@sveltejs/adapter-static';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	kit: {
		adapter: adapter({
			pages: 'build',
			assets: 'build',
			fallback: 'index.html',
			// Write .br and .gz next to every text asset; tond-server serves them
			// with Content-Encoding (see static_files.rs).
			precompress: true
		})
	}
};

export default config;
