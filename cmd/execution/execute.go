package execution

import (
	"encoding/json"
	"fmt"
	"github.com/aws/aws-sdk-go/aws"
	"github.com/aws/aws-sdk-go/aws/credentials"
	"github.com/aws/aws-sdk-go/aws/session"
	"github.com/aws/aws-sdk-go/service/sqs"
	"os"
	"runtime"
	"sync"
)

const MaxMessage = 10

type RunningSession struct {
	session  *session.Session
	svc      *sqs.SQS
	messages []*sqs.Message
	wg       *sync.WaitGroup
	QueueArn string
	FileName string
	Profile  string
	Region   string
	pollSize int
}

func (s *RunningSession) initializeSQS() {
	s.svc = sqs.New(s.session)
}

func (s *RunningSession) startPolling() {
	s.fetchMessagesFromSQS()
}

func (s *RunningSession) fetchMessagesFromSQS() {
	curr := 0
	prev := 0
	for true {
		msgResult, err := s.svc.ReceiveMessage(&sqs.ReceiveMessageInput{
			QueueUrl:            &s.QueueArn,
			MaxNumberOfMessages: aws.Int64(MaxMessage),
		})

		if err != nil {
			fmt.Printf("Got error in receiving message: %v\n", err)
			os.Exit(1)
		}

		curr += len(msgResult.Messages)

		for _, item := range msgResult.Messages {
			s.messages = append(s.messages, item)
		}

		if curr == prev {
			break
		}

		prev = curr
	}
	s.wg.Done()
}

func (s *RunningSession) createFile() {
	fmt.Printf("Fetched %d records\n", len(s.messages))
	file, err := json.MarshalIndent(s.messages, "", " ")
	if err != nil {
		fmt.Printf("Error in marshalling data %v\n", err)
		os.Exit(1)
	}
	err = os.WriteFile(s.FileName, file, 0644)
	if err != nil {
		fmt.Printf("Error in writing to file %v\n", err)
		os.Exit(1)
	}
}

func (s *RunningSession) setPool() {
	var wg sync.WaitGroup
	pollSize := runtime.NumCPU()
	wg.Add(pollSize)
	s.pollSize = pollSize
	s.wg = &wg
}

// Perform executes the root command to purge the SQS
func (s *RunningSession) Perform() {
	s.setPool()
	s.createAWSSession()
	s.initializeSQS()
	s.triggerPollers()
	s.wg.Wait()
	s.createFile()
}

func (s *RunningSession) triggerPollers() {
	for idx := 0; idx < s.pollSize; idx++ {
		go s.startPolling()
	}
}

func (s *RunningSession) createAWSSession() {
	awsSession, err := session.NewSession(&aws.Config{
		Region:      aws.String(s.Region),
		Credentials: credentials.NewSharedCredentials("", s.Profile),
	})

	if err != nil {
		fmt.Printf("Error received in session creation: %v\n", err)
		os.Exit(1)
	}

	s.session = awsSession
}
