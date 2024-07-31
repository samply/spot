# Samply.Spot

This is the Rust re-implementation of Samply.Spot.

## Local setup
Spot can be run locally with the provided [docker-compose](./docker-compose.yml) which requires a running beam installation. This can be done by cloning the [beam repository](https://github.com/samply/beam) and running `./dev/beamdev demo`

Spot can also be run from command line stating command line parameters, like so:
```bash
cargo run -- --beam-proxy-url http://localhost:8081 --beam-app-id app1.proxy1.broker --beam-secret App1Secret --cors-origin any --bind-addr 127.0.0.1:8055 --catalogue-url https://raw.githubusercontent.com/samply/lens/main/packages/demo/public/catalogues/catalogue-dktk.json
```

The following environment variables are mandatory for the usage of Spot.
```
--beam-proxy-url <BEAM_PROXY_URL>
    URL of the Beam Proxy, e.g. https://proxy1.broker.samply.de [env: BEAM_PROXY_URL=]
--beam-app-id <BEAM_APP_ID>
    Beam AppId of this application, e.g. spot.proxy1.broker.samply.de [env: BEAM_APP_ID=]
--beam-secret <BEAM_SECRET>
    Credentials to use on the Beam Proxy [env: BEAM_SECRET=]
--cors-origin <CORS_ORIGIN>
   Where to allow cross-origin resourse sharing from [env: CORS_ORIGIN=]
```

Optional environment variables:
```
--project <PROJECT>
    Optional project name used by focus [env: PROJECT=]
--catalogue-url <CATALOGUE_URL>
    URL to catalogue.json file, if not stated, /catalogue endpoint is disabled [env: CATALOGUE_URL=]
--prism-url <PRISM_URL>
    URL to prism, if catalogue-url is not stated, this is never used [env: PRISM_URL=] [default: http://localhost:8066]
--bind-addr <BIND_ADDR>
    The socket address this server will bind to [env: BIND_ADDR=] [default: 0.0.0.0:8055]
```

## API

### /beam
The `/beam` endpoint provides the ability to communicate with the beam-broker through a locally hosted beam-proxy (See [local setup](#local-setup)).
#### POST
With a post to `/beam` you will create a new beam task. You need to send a payload with this structure:

```json
{
    "id": "<a-uuid-to-later-identify-the-task>",
    "sites": [
        "list",
        "of",
        "available",
        "sites"
    ],
    "query": "The query which the receiving site should execute"
}
```

See the example [call](./docs/create-beam-task.sh) in our docs.

##### Success
When executing the query successfully, spot will return a `201` status code with the beam task id in the location header

``` http
HTTP/1.1 201 Created
Location: /beam/<some-uuid>
Content-Length: 0
Date: Mon, 15 Mai 2023 13:00:00 GMT
```

#### GET
The get endpoint takes a beam task id in the path.

``` shell
curl http://localhost:8100/beam/<some-uuid>
```

See the example [call](./docs/listen-for-beam-results.sh)

### /catalogue

#### GET

``` shell
curl http://localhost:8100/catalogue
```

Returns JSON catalogue of search criteria along with metadata fetched from Samply.Prism (see [Prism's repository](https://github.com/samply/prism) for a list of metadata fields).


## License

This code is licensed under the Apache License 2.0. For details, please see [LICENSE](./LICENSE)

