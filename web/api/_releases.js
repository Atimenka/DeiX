'use strict';

const BUCKET = 'os-images';
const MAX_SIZE = 50 * 1024 * 1024;
const VERSION_RE = /^[0-9A-Za-z][0-9A-Za-z._-]{0,31}$/;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const EXT_RE = /\.(img|img\.gz|img\.xz|zip|qcow2)$/i;

/** Путь объекта в бакете: версия проверена VERSION_RE, поэтому кодировать нечего. */
function objectPath(version, fileName) {
  return `${version}/${fileName}`;
}

module.exports = { BUCKET, EXT_RE, MAX_SIZE, UUID_RE, VERSION_RE, objectPath };
