#!/bin/bash

while IFS= read -r line; do
  echo "$line" >> /tmp/a
  echo "$line"
done < "$1"
