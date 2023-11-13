# Samply.Spot

This is the Rust re-implementation of Samply.Spot.

## Local setup
Spot can be run locally with the provided [docker-compose](./docker-compose.yml) which requires a running beam installation. This can be done by cloning the [beam repository](https://github.com/samply/beam) and running `./dev/beamdev demo`

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
The get endpoint takes an beam task id in the path.

``` shell
curl http://localhost:8100/beam/<some-uuid>
```

See the example [call](./docs/listen-for-beam-results.sh)

