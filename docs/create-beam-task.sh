query=$(cat ./example-query.json | base64 | tr -d '\n')
# sudo apt-get install uuid-runtime
uuid="$(uuidgen)"
curl -v -X POST http://localhost:8100/beam \
     -H "Content-Type: application/json" \
     --data-binary @- << EOF
{
  "id": "$uuid",
  "sites": ["proxy2"],
  "query": "$query"
}
EOF
