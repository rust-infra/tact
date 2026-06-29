#!/usr/bin/env node
const http = require('http');
const server = http.createServer((req, res) => {
  res.writeHead(200, { 'Content-Type': 'text/plain' });
  res.end(`Hello from Node ${process.version}\n`);
});
server.listen(3000, () => console.log('Listening on :3000'));
