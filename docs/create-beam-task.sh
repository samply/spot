query=$(cat ./example-query.json | base64 | tr -d '\n')
uuid="$(uuidgen)"
curl -v -X POST http://localhost:8080/beam \
     -H "Content-Type: application/json" \
     --data-binary @- << EOF
{
  "id": "$uuid",
  "sites": ["mannheim"],
  "query": "$query"
}
EOF
