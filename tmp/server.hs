{-# LANGUAGE OverloadedStrings #-}

-- A basic JSON API server using Scotty
import Web.Scotty
import Data.Aeson (object, (.=), Value)
import qualified Data.Text.Lazy as TL

main :: IO ()
main = scotty 8080 $ do
    get "/health" $ do
        json $ object ["status" .= ("ok" :: String)]

    get "/echo/:msg" $ do
        msg <- param "msg"
        json $ object ["echo" .= msg]

    get "/time" $ do
        json $ object ["server" .= ("Scotty" :: String)]
