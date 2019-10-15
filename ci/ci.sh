#!/bin/bash
set -ex

if [ "${TRAVIS_OS_NAME}" = "osx" ];
then
  brew update
  brew install yarn
fi

npm test

COMMIT_MESSAGE=$(git log --format=%B --no-merges -n 1 | tr -d '\n')
if [[ "${COMMIT_MESSAGE}" =~ "[publish binary]" ]];
then
    yarn upload-binary
fi;
