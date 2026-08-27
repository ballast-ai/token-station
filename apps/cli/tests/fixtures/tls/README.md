# TLS test fixtures

Test-only material (no real secrets). A private test CA and a leaf server
certificate for `localhost` / `127.0.0.1` signed by it, valid ~100 years.
Used by the egress `extra_ca_file` trust tests.

Regenerate:

```sh
openssl req -x509 -newkey rsa:2048 -nodes -keyout ca_key.pem -out ca_cert.pem \
  -days 36500 -subj "/CN=token-station-test-ca" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign"
openssl req -newkey rsa:2048 -nodes -keyout server_key.pem -out server.csr \
  -subj "/CN=localhost"
printf 'basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n' > server.ext
openssl x509 -req -in server.csr -days 36500 -CA ca_cert.pem -CAkey ca_key.pem \
  -CAcreateserial -extfile server.ext -out server_cert.pem
rm server.csr server.ext ca_cert.srl
```
